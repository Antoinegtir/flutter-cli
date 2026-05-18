//! `fl clean` — wraps `flutter clean`, with before/after byte counting.

use anyhow::anyhow;
use fl_core::CleanEvent;
use fl_flutter::resolve_flutter;
use fl_tui::{CleanView, TuiRunner};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tokio::sync::mpsc;

const CLEAN_PATHS: &[&str] = &["build", ".dart_tool", ".flutter-plugins", ".flutter-plugins-dependencies"];

pub async fn run(project: Option<PathBuf>) -> anyhow::Result<()> {
    let project = project.unwrap_or_else(|| std::env::current_dir().unwrap());
    let flutter = resolve_flutter(None, std::env::var("FLUTTER_ROOT").ok().as_deref(), dirs_home())
        .ok_or_else(|| anyhow!("flutter binary not found"))?;

    let (tx, mut rx) = mpsc::channel::<CleanEvent>(32);

    let project_dir = project.clone();
    let flutter_path = flutter.clone();
    tokio::spawn(async move {
        tx.send(CleanEvent::Probing).await.ok();
        let mut total_before: u64 = 0;
        let mut cleaned_paths = Vec::new();
        for rel in CLEAN_PATHS {
            let p = project_dir.join(rel);
            if p.exists() {
                let size = dir_size(&p).await;
                total_before += size;
                cleaned_paths.push(rel.to_string());
                tx.send(CleanEvent::Cleaning { path: rel.to_string() }).await.ok();
            }
        }
        // Run `flutter clean` to also clear native build state.
        let _ = Command::new(&flutter_path)
            .current_dir(&project_dir)
            .args(["clean"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        let mut total_after: u64 = 0;
        for rel in CLEAN_PATHS {
            let p = project_dir.join(rel);
            if p.exists() {
                total_after += dir_size(&p).await;
            }
        }
        tx.send(CleanEvent::Done {
            freed_bytes: total_before.saturating_sub(total_after),
            paths: cleaned_paths,
        }).await.ok();
    });

    if std::env::var_os("FL_HEADLESS").is_some() {
        return drain_headless(rx).await;
    }

    let mut view = CleanView::new();
    let mut runner = TuiRunner::init()?;
    let result = runner.run_view(&mut view, &mut rx).await;
    let _ = runner.restore();
    result
}

/// Best-effort recursive size in bytes. Silently skips errors.
async fn dir_size(path: &Path) -> u64 {
    let mut total: u64 = 0;
    let Ok(mut rd) = tokio::fs::read_dir(path).await else {
        return 0;
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        match entry.metadata().await {
            Ok(md) if md.is_file() => total = total.saturating_add(md.len()),
            Ok(md) if md.is_dir() => {
                let sub = Box::pin(dir_size(&entry.path())).await;
                total = total.saturating_add(sub);
            }
            _ => {}
        }
    }
    total
}

async fn drain_headless(mut rx: mpsc::Receiver<CleanEvent>) -> anyhow::Result<()> {
    while let Some(ev) = rx.recv().await {
        println!("CLEAN {ev:?}");
        if matches!(ev, CleanEvent::Done { .. } | CleanEvent::Error(_)) {
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
