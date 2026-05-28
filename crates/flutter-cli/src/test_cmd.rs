//! `flutter-cli test` — wraps `flutter test --machine` with a live TUI that
//! survives test completion and supports re-running with `r`.

use anyhow::{anyhow, Context};
use fl_adb::{parse_devices_l, CommandRunner, TokioRunner};
use fl_flutter::{parse_test_line, resolve_flutter};
use fl_tui::{TestView, TuiRunner};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

/// All the `flutter-cli test` options bundled up. Threaded through to the
/// flutter spawner where each field is translated into the matching
/// `flutter test` CLI flag.
pub struct Options {
    pub project: Option<PathBuf>,
    pub device: Option<String>,
    pub name: Option<String>,
    pub plain_name: Option<String>,
    pub tags: Vec<String>,
    pub exclude_tags: Vec<String>,
    pub coverage: bool,
    pub update_goldens: bool,
    /// `--golden`: shorthand that defaults `paths` to `test/golden/`
    /// when no explicit paths were given. Mirrors the convention used
    /// in Makefile setups where `make golden` / `make golden-update`
    /// run only the goldens-suite.
    pub golden: bool,
    pub reporter: Option<String>,
    pub concurrency: Option<u32>,
    pub paths: Vec<String>,
    pub extra: Vec<String>,
}

/// Handle returned by `spawn_flutter_test`. The wrapper task owns the
/// child process and the stdout/stderr readers; the caller signals
/// "please kill the child" by dropping this (the oneshot sender) or
/// calling `request_kill`, then awaits the join handle to be sure the
/// process is reaped before spawning a new one.
struct TestRun {
    kill: Option<tokio::sync::oneshot::Sender<()>>,
    join: tokio::task::JoinHandle<()>,
}

impl TestRun {
    /// Best-effort kill of the running `flutter test` child. Idempotent.
    fn request_kill(&mut self) {
        if let Some(tx) = self.kill.take() {
            let _ = tx.send(());
        }
    }

    /// Wait for the wrapper task — and therefore the child — to finish.
    async fn finish(mut self) {
        self.request_kill();
        let _ = self.join.await;
    }
}

pub async fn run(opts: Options) -> anyhow::Result<()> {
    let project = opts
        .project
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    if !project.join("pubspec.yaml").exists() {
        return Err(anyhow!("no pubspec.yaml in {}", project.display()));
    }

    // Pre-test hooks (codegen, fixture seeding, etc.) run BEFORE the
    // inline TUI initializes so their output stays in the scrollback.
    crate::config::run_pre_hooks("test", &project).await?;
    // `--golden` is shorthand for `flutter-cli test test/golden/`. Resolve it
    // before any path validation so the "needs `test/`" check below
    // can be skipped when goldens live elsewhere isn't a concern.
    let mut opts = opts;
    if opts.golden && opts.paths.is_empty() {
        opts.paths.push("test/golden/".into());
    }
    // We used to require `test/` to exist, but `flutter-cli test integration_test/`
    // (or any custom path the user explicitly types) is perfectly valid
    // for a project that only ships integration tests. Only enforce the
    // default-`test/`-must-exist rule when no paths were given.
    if opts.paths.is_empty() && !project.join("test").is_dir() {
        return Err(anyhow!(
            "no `test/` directory in {} — pass an explicit path \
             (e.g. `flutter-cli test integration_test/`) if your tests live elsewhere",
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

    // If the user is running integration / e2e tests and didn't pin a
    // device, `flutter test` will either auto-pick (one device) or
    // prompt interactively (multiple devices). Our stdin is detached
    // for terminal hygiene, so the interactive prompt deadlocks — we
    // pre-resolve a device here using the same picker as `flutter-cli run`.
    if opts.device.is_none() && paths_need_device(&opts.paths) {
        if let Some(serial) = pick_device_for_integration().await? {
            opts.device = Some(serial);
        }
    }

    if std::env::var_os("FL_HEADLESS").is_some() {
        let (tx, rx) = mpsc::channel::<fl_core::TestEvent>(128);
        let run = spawn_flutter_test(&flutter, &project, &opts, tx)?;
        let r = drain_headless(rx).await;
        run.finish().await;
        return r;
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
        let run = match spawn_flutter_test(&flutter, &project, &opts, tx) {
            Ok(r) => r,
            Err(e) => break Err(e),
        };

        let run_result = runner.run_view(&mut view, &mut rx).await;

        // The view said it wants to stop. Make sure the test process
        // is actually dead before we either restart or exit — we
        // don't want orphan `flutter test` processes lingering.
        run.finish().await;

        if view.wants_restart {
            view = TestView::new();
            continue;
        }
        break run_result;
    };

    let _ = runner.restore();
    result
}

/// Spawn `flutter test --machine [filters/paths/...]` and pipe its
/// parsed events into `tx`. Unparsed stdout lines and the entire
/// stderr stream are surfaced as `TestEvent::Error` so the user
/// actually sees what flutter is doing when the run fails early
/// (e.g. "no device available" for integration tests, missing
/// `dev_dependency`, compilation errors, …). When the child exits
/// without ever emitting an `AllDone` event we synthesize one based
/// on the exit code — without this, the TUI hangs forever because
/// `TestView::quitting()` only flips on `AllDone`.
fn spawn_flutter_test(
    flutter: &Path,
    project: &Path,
    opts: &Options,
    tx: mpsc::Sender<fl_core::TestEvent>,
) -> anyhow::Result<TestRun> {
    // Always start with `--machine` so we get the structured JSON
    // event stream that `TestView` knows how to parse. Everything else
    // is layered on top.
    let mut args: Vec<String> = vec!["test".into(), "--machine".into()];

    if let Some(d) = &opts.device {
        args.push("-d".into());
        args.push(d.clone());
    }

    // Filters.
    if let Some(n) = &opts.name {
        args.push("--name".into());
        args.push(n.clone());
    }
    if let Some(n) = &opts.plain_name {
        args.push("--plain-name".into());
        args.push(n.clone());
    }
    for t in &opts.tags {
        args.push("--tags".into());
        args.push(t.clone());
    }
    for t in &opts.exclude_tags {
        args.push("--exclude-tags".into());
        args.push(t.clone());
    }

    // Behavioural toggles.
    if opts.coverage {
        args.push("--coverage".into());
    }
    if opts.update_goldens {
        args.push("--update-goldens".into());
    }
    if let Some(r) = &opts.reporter {
        args.push("--reporter".into());
        args.push(r.clone());
    }
    if let Some(c) = opts.concurrency {
        args.push(format!("--concurrency={c}"));
    }

    // Escape hatch: anything the user passed after `--`.
    for a in &opts.extra {
        args.push(a.clone());
    }

    // Positional test paths LAST — `flutter test [flags] <paths...>`.
    for p in &opts.paths {
        args.push(p.clone());
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

    let stdout = child.stdout.take().context("flutter test stdout pipe")?;
    let stderr = child.stderr.take().context("flutter test stderr pipe")?;

    // Shared flag so the exit-watcher knows whether the readers
    // already saw an `AllDone` event — in which case we don't need to
    // synthesize one and risk a double-completion in the view.
    let saw_all_done = Arc::new(AtomicBool::new(false));

    // STDOUT reader: parse JSON lines from `--machine`. Anything else
    // is dropped — `flutter test --machine` also prints things like
    // "Xcode build done." and group-definition JSON our parser
    // doesn't decode; routing those into TestView's Failures panel
    // produced a wall of noise. Genuine errors come via stderr below.
    let tx_out = tx.clone();
    let flag_out = saw_all_done.clone();
    let stdout_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(ev) = parse_test_line(&line) {
                if matches!(ev, fl_core::TestEvent::AllDone { .. }) {
                    flag_out.store(true, Ordering::SeqCst);
                }
                if tx_out.send(ev).await.is_err() {
                    break;
                }
            }
        }
    });

    // STDERR reader: every line goes through as an Error event. Flutter
    // tends to use stderr for fatal startup messages ("No supported
    // devices connected.", "Target file 'integration_test/' not found.",
    // tool-exit lines, …).
    let tx_err = tx.clone();
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if tx_err
                .send(fl_core::TestEvent::Error {
                    id: None,
                    message: trimmed.to_string(),
                    stack: None,
                })
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Wrapper task: holds the child, races the kill signal against the
    // child's natural exit, drains the readers, and emits a synthetic
    // `AllDone` when needed so the view can quit.
    let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();
    let join = tokio::spawn(async move {
        let mut child = child;
        let exit_status = tokio::select! {
            biased;
            _ = kill_rx => {
                let _ = child.start_kill();
                child.wait().await.ok()
            }
            status = child.wait() => status.ok(),
        };

        // Let the readers finish flushing whatever is left in the
        // pipes after the child exits.
        let _ = stdout_task.await;
        let _ = stderr_task.await;

        if !saw_all_done.load(Ordering::SeqCst) {
            let success = exit_status.as_ref().map(|s| s.success()).unwrap_or(false);
            // If the child died before any test event was reported,
            // make the failure mode obvious in the view.
            if let Some(code) = exit_status.as_ref().and_then(|s| s.code()) {
                if code != 0 {
                    let _ = tx
                        .send(fl_core::TestEvent::Error {
                            id: None,
                            message: format!(
                                "flutter test exited with code {code} \
                                 — see error lines above"
                            ),
                            stack: None,
                        })
                        .await;
                }
            }
            let _ = tx
                .send(fl_core::TestEvent::AllDone {
                    success,
                    passed: 0,
                    failed: 0,
                    skipped: 0,
                })
                .await;
        }
    });

    Ok(TestRun {
        kill: Some(kill_tx),
        join,
    })
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

/// True when any of the user-provided paths looks like an integration
/// test entry point — these run on a real device/sim/desktop and need
/// `-d <id>`, unlike unit/widget tests which run on the host VM.
///
/// We treat anything under (or named) `integration_test` as needing a
/// device. That covers `flutter-cli test integration_test/`,
/// `flutter-cli test integration_test/login_test.dart`, and the convention
/// most Flutter projects follow.
fn paths_need_device(paths: &[String]) -> bool {
    paths.iter().any(|p| {
        let p = p.trim_start_matches("./").trim_end_matches('/');
        p == "integration_test" || p.starts_with("integration_test/")
    })
}

/// Enumerate connected devices (Android via `adb`, iOS via
/// `xcrun devicectl`, macOS desktop) and, if there are multiple, show
/// the same picker as `flutter-cli run`. Returns the chosen serial, or `None`
/// when no devices are connected at all (we let flutter print its
/// own "no devices" error in that case).
async fn pick_device_for_integration() -> anyhow::Result<Option<String>> {
    let runner = TokioRunner;
    let mut devices: Vec<fl_core::Device> = Vec::new();
    if let Ok(out) = runner.run("adb", &["devices", "-l"]).await {
        devices.extend(parse_devices_l(&out.stdout));
    }
    // `xcrun` only exists on macOS — skip the spawn entirely elsewhere
    // so Linux/Windows builds don't pay for two ENOENT'ing child procs
    // per invocation. `cfg!` is const-evaluated so the unreached branch
    // is dropped at compile time.
    if cfg!(target_os = "macos") {
        let xcrun = fl_ios::Xcrun::new(TokioRunner);
        devices.extend(fl_ios::list_apple_devices(&xcrun).await);
    }
    // Only consider devices that are actually usable.
    devices.retain(|d| matches!(d.state, fl_core::DeviceState::Online));
    match devices.len() {
        0 => Ok(None),
        1 => Ok(Some(devices[0].serial.clone())),
        _ => {
            let picked = crate::multi::run_picker(&devices).await?;
            // The picker returns a Vec because the `flutter-cli run` flow
            // supports multiple devices in parallel. For integration
            // tests we just take the first selected one — flutter
            // test only drives a single device per invocation.
            Ok(picked.into_iter().next())
        }
    }
}

fn dirs_home() -> Option<&'static Path> {
    static HOME: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()))
        .as_deref()
}
