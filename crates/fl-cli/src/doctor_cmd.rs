//! `fl doctor` — wraps `flutter doctor -v`, streams sections.

use anyhow::{anyhow, Context};
use fl_core::DoctorEvent;
use fl_flutter::{parse_doctor_output, resolve_flutter};
use fl_tui::{DoctorView, TuiRunner};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;

pub async fn run() -> anyhow::Result<()> {
    let flutter = resolve_flutter(None, std::env::var("FLUTTER_ROOT").ok().as_deref(), dirs_home())
        .ok_or_else(|| anyhow!("flutter binary not found"))?;

    let (tx, mut rx) = mpsc::channel::<DoctorEvent>(32);

    let flutter_path = flutter.clone();
    tokio::spawn(async move {
        let mut child = Command::new(&flutter_path)
            .args(["doctor", "-v"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawn flutter doctor")
            .expect("spawn");
        let mut stdout = child.stdout.take().expect("stdout");
        let mut buf = String::new();
        stdout.read_to_string(&mut buf).await.ok();
        let _ = child.wait().await;
        for ev in parse_doctor_output(&buf) {
            tx.send(ev).await.ok();
        }
    });

    if std::env::var_os("FL_HEADLESS").is_some() {
        return drain_headless(rx).await;
    }

    let mut view = DoctorView::new();
    let mut runner = TuiRunner::init()?;
    let result = runner.run_view(&mut view, &mut rx).await;
    let _ = runner.restore();
    result
}

async fn drain_headless(mut rx: mpsc::Receiver<DoctorEvent>) -> anyhow::Result<()> {
    while let Some(ev) = rx.recv().await {
        println!("DOC {ev:?}");
        if matches!(ev, DoctorEvent::Done) {
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
