//! `fl build <target> [--mode]` — wraps `flutter build <target> --machine`.

use anyhow::{anyhow, Context};
use fl_core::BuildMode;
use fl_flutter::{parse_daemon_line, resolve_flutter};
use fl_tui::{BuildView, TuiRunner};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

pub async fn run(
    target: String,
    project: Option<PathBuf>,
    mode: BuildMode,
    extra: Vec<String>,
) -> anyhow::Result<()> {
    let project = project.unwrap_or_else(|| std::env::current_dir().unwrap());
    if !project.join("pubspec.yaml").exists() {
        return Err(anyhow!(
            "no pubspec.yaml in {} — not a Flutter project",
            project.display()
        ));
    }
    let flutter = resolve_flutter(
        None,
        Some(project.as_path()),
        std::env::var("FLUTTER_ROOT").ok().as_deref(),
        dirs_home(),
    )
    .ok_or_else(|| anyhow!("flutter binary not found"))?;

    let (tx, mut rx) = mpsc::channel::<fl_core::FlutterEvent>(128);

    // The target string is whatever the user typed (apk, ios, ipa,
    // macos, …). We forward it verbatim — `flutter build` will reject
    // unknown subcommands itself and we capture its stderr in the TUI.
    let target_arg = target.clone();
    let mode_flag = mode.flutter_flag().to_string();
    let project_dir = project.clone();
    let flutter_path = flutter.clone();

    let extra_owned = extra;
    tokio::spawn(async move {
        // We used to pass `--machine` to get JSON progress events, but
        // only `flutter build apk/appbundle/aar` honor it; `build ios`
        // exits immediately with an arg-parse failure (which the user
        // saw as an empty Steps panel and a 0.1s build). Plain text
        // output is fine — we stream every line into the TUI as a log
        // and the build view shows the tail.
        let mut args: Vec<&str> = vec!["build", &target_arg];
        if !matches!(mode, BuildMode::Release) {
            args.push(&mode_flag);
        }
        // User pass-through args (after `--` on the `fl build` line):
        // `--flavor`, `--target=lib/main_prod.dart`, `--obfuscate`, etc.
        for a in &extra_owned {
            args.push(a.as_str());
        }
        // Spawn defensively: a failure here used to panic via `.expect`
        // because the TUI was already attached, leaving the terminal in
        // a half-drawn state. Now we surface the error as a Log + a
        // synthetic Stopped event so the TUI exits cleanly and the user
        // sees what went wrong.
        let mut child = match Command::new(&flutter_path)
            .current_dir(&project_dir)
            .args(&args)
            // Detach stdin so the child can't steal mouse-tracking
            // bytes from the TTY (see test_cmd.rs for the full story).
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning flutter build")
        {
            Ok(c) => c,
            Err(e) => {
                tx.send(fl_core::FlutterEvent::Log {
                    level: fl_core::LogLevel::Error,
                    message: format!("flutter build failed to start: {e:#}"),
                })
                .await
                .ok();
                tx.send(fl_core::FlutterEvent::Stopped { exit_code: Some(1) })
                    .await
                    .ok();
                return;
            }
        };
        let Some(stdout) = child.stdout.take() else {
            tx.send(fl_core::FlutterEvent::Log {
                level: fl_core::LogLevel::Error,
                message: "flutter build: stdout pipe unavailable".into(),
            })
            .await
            .ok();
            tx.send(fl_core::FlutterEvent::Stopped { exit_code: Some(1) })
                .await
                .ok();
            return;
        };
        let Some(stderr) = child.stderr.take() else {
            tx.send(fl_core::FlutterEvent::Log {
                level: fl_core::LogLevel::Error,
                message: "flutter build: stderr pipe unavailable".into(),
            })
            .await
            .ok();
            tx.send(fl_core::FlutterEvent::Stopped { exit_code: Some(1) })
                .await
                .ok();
            return;
        };

        let tx_out = tx.clone();
        let out_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(ev) = parse_daemon_line(&line) {
                    tx_out.send(ev).await.ok();
                } else {
                    tx_out
                        .send(fl_core::FlutterEvent::Log {
                            level: fl_core::LogLevel::Debug,
                            message: line,
                        })
                        .await
                        .ok();
                }
            }
        });

        let tx_err = tx.clone();
        let err_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tx_err
                    .send(fl_core::FlutterEvent::Log {
                        level: fl_core::LogLevel::Error,
                        message: line,
                    })
                    .await
                    .ok();
            }
        });

        let status = child.wait().await.unwrap_or_default();
        // `child.wait()` returns as soon as the OS process exits, but
        // the stdout/stderr reader tasks may still be draining buffered
        // bytes from the pipes. If we emit `Stopped` before they finish,
        // a headless consumer (or the TUI) can race past the tail
        // events — including the very `Built …` log line we want to
        // surface. Join the readers first so they flush in order.
        let _ = out_task.await;
        let _ = err_task.await;
        tx.send(fl_core::FlutterEvent::Stopped {
            exit_code: status.code(),
        })
        .await
        .ok();
    });

    if std::env::var_os("FL_HEADLESS").is_some() {
        return drain_headless(rx).await;
    }

    let mut view = BuildView::new(target, mode);
    // Inline viewport so the user's shell history stays visible above
    // the build dashboard — same UX as `fl run`. ~14 rows is enough for
    // the build status (target, mode, progress, last-error) without
    // crowding the scrollback. If anything goes wrong while attaching
    // (raw mode unavailable, stdout not a TTY, …) we report it as an
    // error instead of crashing the process with the daemon still
    // running in the background.
    let mut runner = match TuiRunner::init_inline(14) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("fl build: could not attach inline TUI: {e:#}");
            return Err(e);
        }
    };
    let result = runner.run_view(&mut view, &mut rx).await;
    let _ = runner.restore();
    result
}

async fn drain_headless(mut rx: mpsc::Receiver<fl_core::FlutterEvent>) -> anyhow::Result<()> {
    while let Some(ev) = rx.recv().await {
        println!("FLU {ev:?}");
        if matches!(ev, fl_core::FlutterEvent::Stopped { .. }) {
            break;
        }
    }
    Ok(())
}

fn dirs_home() -> Option<&'static Path> {
    static HOME: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()))
        .as_deref()
}
