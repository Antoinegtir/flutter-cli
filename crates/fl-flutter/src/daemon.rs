//! Spawn `flutter run --machine` and forward parsed events through a channel.

use crate::parse::parse_daemon_line;
use anyhow::Context;
use fl_core::FlutterEvent;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc::Sender;

pub struct FlutterDaemon {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl FlutterDaemon {
    /// Spawn `flutter attach --machine -d <device_id> --debug-url <ws> [extra_args]`
    /// and stream events. Used by the Wi-Fi takeover flow: after a USB
    /// unplug we attach a fresh daemon to the still-running VM Service
    /// over Apple's coredevice tunnel. The new daemon owns the compile
    /// pipeline (frontend_server) so hot reload actually applies new
    /// source code instead of just calling `reloadSources` on stale
    /// bytecode.
    pub async fn spawn_attach(
        flutter: &Path,
        project_dir: &Path,
        device_id: &str,
        debug_url: &str,
        extra_args: &[&str],
        tx: Sender<FlutterEvent>,
    ) -> anyhow::Result<Self> {
        // Run in verbose mode so we get the internal Flutter / lldb /
        // devicectl chatter on stderr. We need this to diagnose why
        // the post-unplug attach fails — without `-v` Flutter swallows
        // the actual transport / debugserver errors and emits only the
        // generic "Connecting to the VM Service is taking longer than
        // expected" message.
        let mut args: Vec<&str> =
            vec!["-v", "attach", "--machine", "-d", device_id, "--debug-url", debug_url];
        args.extend_from_slice(extra_args);

        // Resolve the iOS SDK root and inject as SDKROOT. The Flutter
        // `native_assets` build target (used during incremental
        // compiles for FFI-shipped native libraries) demands a
        // `SdkRoot` define; when `flutter attach` runs outside of an
        // xcodebuild context, no SDKROOT env var is set and the
        // target fails with "required define SdkRoot but it was not
        // provided" — which kills the post-unplug hot reload.
        //
        // We *unconditionally* (re)set SDKROOT to whatever `xcrun
        // --show-sdk-path` returns now. A stale value can leak in
        // from the parent shell (e.g. a previous `flutter run` that
        // left SDKROOT pointed at iPhoneSimulator's SDK) and we want
        // to be authoritative.
        let mut env: Vec<(String, String)> = Vec::new();
        let probe = tokio::process::Command::new("xcrun")
            .args(["--sdk", "iphoneos", "--show-sdk-path"])
            .output()
            .await;
        if let Ok(out) = probe {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() {
                env.push(("SDKROOT".to_string(), path));
            }
        }
        Self::spawn_with_args(flutter, project_dir, &args, &env, tx).await
    }

    /// Spawn `flutter run --machine -d <device_id> [extra_args]` and stream events into `tx`.
    pub async fn spawn(
        flutter: &Path,
        project_dir: &Path,
        device_id: &str,
        extra_args: &[&str],
        tx: Sender<FlutterEvent>,
    ) -> anyhow::Result<Self> {
        let mut args: Vec<&str> = vec!["run", "--machine", "-d", device_id];
        args.extend_from_slice(extra_args);
        Self::spawn_with_args(flutter, project_dir, &args, &[], tx).await
    }

    async fn spawn_with_args(
        flutter: &Path,
        project_dir: &Path,
        args: &[&str],
        env: &[(String, String)],
        tx: Sender<FlutterEvent>,
    ) -> anyhow::Result<Self> {
        let mut cmd = Command::new(flutter);
        cmd.current_dir(project_dir)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().context("spawning flutter")?;

        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;
        let stdin = child.stdin.take();

        let tx_out = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(ev) = parse_daemon_line(&line) {
                    tx_out.send(ev).await.ok();
                } else if line.starts_with('[') {
                    // Unhandled daemon JSON event — drop silently rather
                    // than dumping the raw 1000-char payload to the log.
                    continue;
                } else {
                    tx_out.send(FlutterEvent::Log {
                        level: fl_core::LogLevel::Debug,
                        message: truncate_log_line(line),
                    }).await.ok();
                }
            }
        });

        let tx_err = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tx_err.send(FlutterEvent::Log {
                    level: classify_stderr_line(&line),
                    message: truncate_log_line(line),
                }).await.ok();
            }
        });

        Ok(Self { child, stdin })
    }
}

/// Decide the log level for a stderr line from the Flutter daemon.
///
/// Flutter's CLI writes a lot of *informational* output to stderr —
/// progress hints, sync chatter, harmless retries — alongside actual
/// errors. Painting all of it red ("ERROR [00008140] Waiting for
/// another flutter command to release the startup lock…") made the
/// multi-device dashboard look broken even on a perfectly healthy
/// run. We pattern-match the few status messages we recognize and
/// down-rank them to Info / Warn so genuine errors still stand out.
fn classify_stderr_line(line: &str) -> fl_core::LogLevel {
    // Startup-lock contention: Flutter serializes parallel `flutter`
    // invocations behind a global lock. The second device waits — it's
    // expected behaviour when N devices fire up in parallel.
    if line.contains("Waiting for another flutter command to release the startup lock") {
        return fl_core::LogLevel::Info;
    }
    // Wireless-debug nudge: noisy but not actionable. The user already
    // sees the Wi-Fi pre-pair status banner elsewhere.
    if line.contains("Wireless debugging on iOS") {
        return fl_core::LogLevel::Warn;
    }
    fl_core::LogLevel::Error
}

/// Cap a log line at 500 chars so ratatui doesn't have to allocate / clamp
/// gigantic strings every render. Appends `…` when truncation happens.
fn truncate_log_line(mut line: String) -> String {
    const MAX: usize = 500;
    if line.chars().count() <= MAX {
        return line;
    }
    line = line.chars().take(MAX - 1).collect();
    line.push('…');
    line
}

impl FlutterDaemon {

    /// Send `q` to the daemon to gracefully quit the running app.
    pub async fn send_quit(&mut self) -> anyhow::Result<()> {
        if let Some(stdin) = self.stdin.as_mut() {
            stdin.write_all(b"q\n").await?;
            stdin.flush().await?;
        }
        Ok(())
    }

    /// Request a hot reload (`full == false`) or hot restart (`full == true`)
    /// via the Flutter daemon's `app.restart` JSON-RPC. This is the
    /// canonical path — VM Service does not expose hot restart directly.
    pub async fn send_app_restart(&mut self, app_id: &str, full: bool) -> anyhow::Result<()> {
        let payload = format!(
            r#"[{{"id":1,"method":"app.restart","params":{{"appId":"{app_id}","fullRestart":{full},"pause":false}}}}]"#
        );
        if let Some(stdin) = self.stdin.as_mut() {
            stdin.write_all(payload.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
        }
        Ok(())
    }

    /// Wait until the child process exits and return its code.
    pub async fn wait(&mut self) -> anyhow::Result<Option<i32>> {
        let status = self.child.wait().await?;
        Ok(status.code())
    }

    /// Force-kill the child.
    pub async fn kill(&mut self) -> anyhow::Result<()> {
        self.child.kill().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// Use a tiny shell script as a "fake flutter" that emits a couple of daemon events then exits.
    fn write_fake_flutter(dir: &Path) -> std::path::PathBuf {
        let script = dir.join("flutter");
        std::fs::write(&script, r#"#!/bin/sh
echo '[{"event":"daemon.connected","params":{"version":"0.6.1"}}]'
echo '[{"event":"app.started","params":{"appId":"abc","vmServiceUri":"ws://127.0.0.1:1/abc/ws"}}]'
echo '[{"event":"app.stopped","params":{"exitCode":0}}]'
"#).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
        script
    }

    #[tokio::test]
    async fn forwards_parsed_events_from_a_fake_flutter() {
        let dir = std::env::temp_dir().join(format!(
            "fl-fake-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = write_fake_flutter(&dir);

        let (tx, mut rx) = mpsc::channel(16);
        let mut daemon = FlutterDaemon::spawn(&exe, &dir, "fake-device", &[], tx).await.unwrap();
        let _ = daemon.wait().await;

        let mut got = Vec::new();
        while let Ok(ev) = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
            if let Some(ev) = ev { got.push(ev); } else { break; }
        }
        assert!(got.iter().any(|e| matches!(e, FlutterEvent::DaemonReady)));
        assert!(got.iter().any(|e| matches!(e, FlutterEvent::AppStarted { .. })));
        assert!(got.iter().any(|e| matches!(e, FlutterEvent::Stopped { exit_code: Some(0) })));
    }
}
