//! `fl test` — wraps `flutter test --machine`.

use anyhow::{anyhow, Context};
use fl_flutter::{parse_test_line, resolve_flutter};
use fl_tui::{TestView, TuiRunner};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

pub async fn run(project: Option<PathBuf>, name_filter: Option<String>) -> anyhow::Result<()> {
    let project = project.unwrap_or_else(|| std::env::current_dir().unwrap());
    if !project.join("pubspec.yaml").exists() {
        return Err(anyhow!("no pubspec.yaml in {}", project.display()));
    }
    if !project.join("test").is_dir() {
        return Err(anyhow!("no test/ directory in {}", project.display()));
    }
    let flutter = resolve_flutter(None, std::env::var("FLUTTER_ROOT").ok().as_deref(), dirs_home())
        .ok_or_else(|| anyhow!("flutter binary not found"))?;

    let (tx, mut rx) = mpsc::channel::<fl_core::TestEvent>(128);

    let project_dir = project.clone();
    let flutter_path = flutter.clone();
    tokio::spawn(async move {
        let mut args: Vec<String> = vec!["test".into(), "--machine".into()];
        if let Some(n) = name_filter {
            args.push("--name".into());
            args.push(n);
        }
        let mut child = Command::new(&flutter_path)
            .current_dir(&project_dir)
            .args(args.iter().map(String::as_str))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning flutter test")
            .expect("spawn");
        let stdout = child.stdout.take().expect("stdout");
        let tx_out = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(ev) = parse_test_line(&line) {
                    tx_out.send(ev).await.ok();
                }
            }
        });
        let _ = child.wait().await;
    });

    if std::env::var_os("FL_HEADLESS").is_some() {
        return drain_headless(rx).await;
    }

    let mut view = TestView::new();
    let mut runner = TuiRunner::init()?;
    let result = runner.run_view(&mut view, &mut rx).await;
    let _ = runner.restore();
    result
}

async fn drain_headless(mut rx: mpsc::Receiver<fl_core::TestEvent>) -> anyhow::Result<()> {
    while let Some(ev) = rx.recv().await {
        println!("TEST {ev:?}");
        if matches!(ev, fl_core::TestEvent::AllDone { .. }) {
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
