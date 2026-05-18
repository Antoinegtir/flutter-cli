//! `fl build <target> [--mode]` — wraps `flutter build <target> --machine`.

use anyhow::{anyhow, Context};
use fl_core::{BuildMode, BuildTarget};
use fl_flutter::{parse_daemon_line, resolve_flutter};
use fl_tui::{BuildView, TuiRunner};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

pub async fn run(target: BuildTarget, project: Option<PathBuf>, mode: BuildMode) -> anyhow::Result<()> {
    let project = project.unwrap_or_else(|| std::env::current_dir().unwrap());
    if !project.join("pubspec.yaml").exists() {
        return Err(anyhow!("no pubspec.yaml in {} — not a Flutter project", project.display()));
    }
    let flutter = resolve_flutter(None, std::env::var("FLUTTER_ROOT").ok().as_deref(), dirs_home())
        .ok_or_else(|| anyhow!("flutter binary not found"))?;

    let (tx, mut rx) = mpsc::channel::<fl_core::FlutterEvent>(128);

    let target_arg = target.flutter_arg().to_string();
    let mode_flag = mode.flutter_flag().to_string();
    let project_dir = project.clone();
    let flutter_path = flutter.clone();

    tokio::spawn(async move {
        let mut args: Vec<&str> = vec!["build", &target_arg, "--machine"];
        if !matches!(mode, BuildMode::Release) {
            args.push(&mode_flag);
        }
        let mut child = Command::new(&flutter_path)
            .current_dir(&project_dir)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning flutter build")
            .expect("spawn");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let tx_out = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(ev) = parse_daemon_line(&line) {
                    tx_out.send(ev).await.ok();
                } else {
                    tx_out.send(fl_core::FlutterEvent::Log {
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
                tx_err.send(fl_core::FlutterEvent::Log {
                    level: fl_core::LogLevel::Error,
                    message: line,
                }).await.ok();
            }
        });

        let status = child.wait().await.unwrap_or_default();
        tx.send(fl_core::FlutterEvent::Stopped { exit_code: status.code() }).await.ok();
    });

    if std::env::var_os("FL_HEADLESS").is_some() {
        return drain_headless(rx).await;
    }

    let mut view = BuildView::new(target, mode);
    let mut runner = TuiRunner::init()?;
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
