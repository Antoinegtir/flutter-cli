//! `fl pub <subcommand>` — wraps `flutter pub *`.

use anyhow::{anyhow, Context};
use fl_core::PubEvent;
use fl_flutter::{parse_deps_json, parse_outdated_table, parse_pub_get, resolve_flutter};
use fl_tui::{PubView, TuiRunner};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::cli::PubSub;

pub async fn run(sub: PubSub, project: Option<PathBuf>) -> anyhow::Result<()> {
    let project = project.unwrap_or_else(|| std::env::current_dir().unwrap());
    if !project.join("pubspec.yaml").exists() {
        return Err(anyhow!("no pubspec.yaml in {}", project.display()));
    }
    let flutter = resolve_flutter(None, std::env::var("FLUTTER_ROOT").ok().as_deref(), dirs_home())
        .ok_or_else(|| anyhow!("flutter binary not found"))?;

    let (tx, mut rx) = mpsc::channel::<PubEvent>(64);

    let project_dir = project.clone();
    let flutter_path = flutter.clone();
    let label_string = sub.label().to_string();
    let sub_clone = sub.clone();
    tokio::spawn(async move {
        let _ = run_sub(&flutter_path, &project_dir, &sub_clone, tx.clone()).await;
        tx.send(PubEvent::Done { success: true }).await.ok();
    });

    if std::env::var_os("FL_HEADLESS").is_some() {
        return drain_headless(rx).await;
    }

    let mut view = match sub {
        PubSub::Outdated => PubView::for_outdated(),
        PubSub::Deps => PubView::for_deps(),
        _ => PubView::for_get_or_upgrade(&label_string),
    };
    let mut runner = TuiRunner::init()?;
    let result = runner.run_view(&mut view, &mut rx).await;
    let _ = runner.restore();
    result
}

async fn run_sub(flutter: &Path, project: &Path, sub: &PubSub, tx: mpsc::Sender<PubEvent>) -> anyhow::Result<()> {
    match sub {
        PubSub::Get => run_text(flutter, project, &["pub", "get"], tx, parse_pub_get_event).await,
        PubSub::Upgrade => run_text(flutter, project, &["pub", "upgrade"], tx, parse_pub_get_event).await,
        PubSub::Outdated => run_text(flutter, project, &["pub", "outdated"], tx, parse_outdated_event).await,
        PubSub::Deps => run_text(flutter, project, &["pub", "deps", "--json"], tx, parse_deps_event).await,
        PubSub::Add { package } => {
            run_text(flutter, project, &["pub", "add", package], tx, parse_pub_get_event).await
        }
        PubSub::Remove { package } => {
            run_text(flutter, project, &["pub", "remove", package], tx, parse_pub_get_event).await
        }
    }
}

async fn run_text<F>(flutter: &Path, project: &Path, args: &[&str], tx: mpsc::Sender<PubEvent>, parse: F) -> anyhow::Result<()>
where F: FnOnce(&str) -> PubEvent {
    let mut child = Command::new(flutter)
        .current_dir(project)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn flutter pub")?;
    let mut stdout = child.stdout.take().expect("stdout");
    let mut buf = String::new();
    stdout.read_to_string(&mut buf).await.ok();
    let _ = child.wait().await;
    let ev = parse(&buf);
    tx.send(ev).await.ok();
    Ok(())
}

fn parse_pub_get_event(s: &str) -> PubEvent { parse_pub_get(s) }
fn parse_outdated_event(s: &str) -> PubEvent { PubEvent::Outdated { rows: parse_outdated_table(s) } }
fn parse_deps_event(s: &str) -> PubEvent {
    match parse_deps_json(s) {
        Ok(tree) => PubEvent::Deps { tree },
        Err(e) => PubEvent::Log { level: fl_core::LogLevel::Error, message: e.to_string() },
    }
}

async fn drain_headless(mut rx: mpsc::Receiver<PubEvent>) -> anyhow::Result<()> {
    while let Some(ev) = rx.recv().await {
        println!("PUB {ev:?}");
        if matches!(ev, PubEvent::Done { .. }) {
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
