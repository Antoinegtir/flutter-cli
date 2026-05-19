//! `fl test` — wraps `flutter test --machine` with a live TUI that
//! survives test completion and supports re-running with `r`.

use anyhow::{anyhow, Context};
use fl_flutter::{parse_test_line, resolve_flutter};
use fl_tui::{TestView, TuiRunner};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
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

    if std::env::var_os("FL_HEADLESS").is_some() {
        let (tx, rx) = mpsc::channel::<fl_core::TestEvent>(128);
        spawn_flutter_test(&flutter, &project, name_filter.as_deref(), tx).await?;
        return drain_headless(rx).await;
    }

    let mut view = TestView::new();
    // Inline viewport (Claude-Code style): the test runner box sits
    // at the bottom of the terminal, leaving the user's shell history
    // visible above. 22 rows fits the tests-list panel + the failures
    // panel + header + footer with room to scroll inside each panel.
    let mut runner = TuiRunner::init_inline(22)?;

    // Outer loop: one iteration per `flutter test` session. When the
    // user presses `r`, the view sets `wants_restart` and we come back
    // around to kill the dying child, reset the view, and spawn again.
    let result: anyhow::Result<()> = loop {
        let (tx, mut rx) = mpsc::channel::<fl_core::TestEvent>(128);
        let mut child = match spawn_flutter_test(&flutter, &project, name_filter.as_deref(), tx).await {
            Ok(c) => c,
            Err(e) => break Err(e),
        };

        let run_result = runner.run_view(&mut view, &mut rx).await;

        // The view said it wants to stop. Make sure the test process
        // is actually dead before we either restart or exit — we
        // don't want orphan `flutter test` processes lingering.
        let _ = child.start_kill();
        let _ = child.wait().await;

        if view.wants_restart {
            view = TestView::new();
            continue;
        }
        break run_result;
    };

    let _ = runner.restore();
    result
}

/// Spawn `flutter test --machine [--name <pattern>]` and pipe its
/// parsed events into `tx`. Returns the `Child` handle so the caller
/// can kill it on re-run.
async fn spawn_flutter_test(
    flutter: &Path,
    project: &Path,
    name_filter: Option<&str>,
    tx: mpsc::Sender<fl_core::TestEvent>,
) -> anyhow::Result<Child> {
    let mut args: Vec<String> = vec!["test".into(), "--machine".into()];
    if let Some(n) = name_filter {
        args.push("--name".into());
        args.push(n.to_string());
    }
    let mut child = Command::new(flutter)
        .current_dir(project)
        .args(args.iter().map(String::as_str))
        // Detach stdin so the child can't steal mouse-tracking bytes
        // from the TTY (`\x1b[<…M`). Without this, scroll events
        // get partially consumed by the dart subprocess and the raw
        // bytes echo back onto our alt-screen.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning flutter test")?;
    let stdout = child.stdout.take().expect("stdout");
    let tx_out = tx.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(ev) = parse_test_line(&line) {
                if tx_out.send(ev).await.is_err() {
                    break;
                }
            }
        }
    });
    Ok(child)
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
