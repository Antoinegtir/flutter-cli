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
        let mut child = Command::new(flutter)
            .current_dir(project_dir)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped())
            .spawn()
            .context("spawning flutter")?;

        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;
        let stdin = child.stdin.take();

        let tx_out = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(ev) = parse_daemon_line(&line) {
                    tx_out.send(ev).await.ok();
                } else {
                    tx_out.send(FlutterEvent::Log {
                        level: fl_core::LogLevel::Debug,
                        message: line,
                    }).await.ok();
                }
            }
        });

        let tx_err = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tx_err.send(FlutterEvent::Log {
                    level: fl_core::LogLevel::Error,
                    message: line,
                }).await.ok();
            }
        });

        Ok(Self { child, stdin })
    }

    /// Send `q` to the daemon to gracefully quit the running app.
    pub async fn send_quit(&mut self) -> anyhow::Result<()> {
        if let Some(stdin) = self.stdin.as_mut() {
            stdin.write_all(b"q\n").await?;
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
