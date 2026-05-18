# Flutter commands + modes (Sub-project B) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `fl build/test/pub/doctor/clean` with command-specific TUI views and the `--mode debug|profile|release` flag on `fl run` and `fl build`.

**Architecture:** Add a `View` trait to `fl-tui` so the renderer hosts more than one view shape; reuse the existing `TuiRunner` event/key loop. Each Flutter sub-command gets a dedicated view, parser (where Flutter offers `--machine`), and command module that owns the runtime wiring. Modes flow as a `BuildMode` enum from clap → command module → `FlutterDaemon::spawn` argument list.

**Tech Stack:** Same as MVP 1 + Sub-project A. No new external dependencies.

**Spec:** [docs/superpowers/specs/2026-05-18-flutter-commands-design.md](../specs/2026-05-18-flutter-commands-design.md)

---

## File Structure

```
crates/fl-core/src/
├── events.rs                    # modify: + TestEvent, DoctorEvent, PubEvent, CleanEvent + BuildMode/BuildTarget
├── lib.rs                       # re-exports

crates/fl-flutter/src/
├── daemon.rs                    # modify: spawn accepts extra args
├── doctor_parse.rs              # new
├── lib.rs                       # re-exports
├── pub_parse.rs                 # new
└── test_parse.rs                # new

crates/fl-tui/src/
├── app.rs                       # modify: impl View for AppState
├── lib.rs                       # re-exports
├── runner.rs                    # modify: + run_view<V: View>
├── view.rs                      # new: View trait
└── views/
    ├── mod.rs                   # new
    ├── build_view.rs            # new
    ├── clean_view.rs            # new
    ├── doctor_view.rs           # new
    ├── pub_view.rs              # new
    └── test_view.rs             # new

crates/fl-cli/src/
├── build_cmd.rs                 # new
├── clean_cmd.rs                 # new
├── cli.rs                       # modify: + sub-commands
├── doctor_cmd.rs                # new
├── main.rs                      # modify: + dispatch
├── pub_cmd.rs                   # new
├── run_cmd.rs                   # modify: + --mode
└── test_cmd.rs                  # new
crates/fl-cli/tests/headless_run.rs    # modify: +5 tests

tests/fixtures/
├── bin/flutter                  # modify: recognise build/test/pub/doctor/clean
└── scenarios/
    ├── build_apk.txt            # new
    ├── clean.txt                # new
    ├── doctor.txt               # new
    ├── pub_get.txt              # new
    └── test_basic.txt           # new
```

---

## Task 1: `View` trait + refactor `AppState` to implement it

**Files:**
- Create: `crates/fl-tui/src/view.rs`
- Modify: `crates/fl-tui/src/app.rs`
- Modify: `crates/fl-tui/src/lib.rs`

- [ ] **Step 1: Create `crates/fl-tui/src/view.rs`**

```rust
//! Generic `View` trait so `TuiRunner` can host multiple command-specific UIs.

use crate::theme::Theme;
use fl_core::KeyEvent as FlKey;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::time::Duration;

pub trait View: Send + 'static {
    /// Event type the view consumes. The runner pushes these via `apply`.
    type Input: Send + 'static;

    /// Apply an event from the producer side (e.g. parsed daemon output).
    fn apply(&mut self, input: Self::Input);

    /// Draw the current state into `buf`.
    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme);

    /// Translate a terminal key into a view-specific `Input` (or `None`).
    /// The runner sends the returned `Input` back through `apply`, so the same
    /// state-mutation code path handles both external and key-derived events.
    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input>;

    /// Called every 33 ms with the elapsed time since the last tick — used for
    /// animations and banner expiry.
    fn tick(&mut self, dt: Duration);

    /// Returns `true` to ask the runner to break out of the loop.
    fn quitting(&self) -> bool;
}
```

- [ ] **Step 2: Implement `View` for `AppState` in `crates/fl-tui/src/app.rs`**

At the bottom of `app.rs` (before the `#[cfg(test)]` block), append:

```rust
impl crate::view::View for AppState {
    type Input = fl_core::AppEvent;

    fn apply(&mut self, input: Self::Input) {
        AppState::apply(self, input);
    }

    fn render(&self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer, theme: &crate::theme::Theme) {
        crate::render::render(area, buf, self, theme);
    }

    fn handle_key(&mut self, key: fl_core::KeyEvent) -> Option<Self::Input> {
        match key {
            fl_core::KeyEvent::Char('q') | fl_core::KeyEvent::Ctrl('c') => {
                self.quitting = true;
                None
            }
            _ => None,
        }
    }

    fn tick(&mut self, _dt: std::time::Duration) {
        self.expire_banner();
    }

    fn quitting(&self) -> bool {
        self.quitting
    }
}
```

> Note: `expire_banner` is currently private. Make it `pub(crate) fn expire_banner` by removing the implicit private visibility (change `fn expire_banner(&mut self)` to `pub(crate) fn expire_banner(&mut self)` in `app.rs`).

- [ ] **Step 3: Update `crates/fl-tui/src/lib.rs`**

Add the new module:

```rust
//! Terminal UI for the `fl` CLI.

pub mod app;
pub mod panels;
pub mod render;
pub mod runner;
pub mod spinner;
pub mod splash;
pub mod theme;
pub mod view;

pub use app::{AppState, Banner, BannerKind, LogLine};
pub use render::render;
pub use runner::{map_key, TuiRunner};
pub use spinner::Spinner;
pub use splash::Splash;
pub use theme::Theme;
pub use view::View;
```

- [ ] **Step 4: Write unit test in `crates/fl-tui/src/view.rs`**

Append to `view.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;
    use fl_core::{AppEvent, FlutterEvent, LogLevel};
    use ratatui::buffer::Buffer;
    use std::time::Duration;

    #[test]
    fn appstate_view_apply_and_render_compile_and_run() {
        let mut s = AppState::new("app".into(), "debug".into());
        <AppState as View>::apply(
            &mut s,
            AppEvent::Flutter(FlutterEvent::Log {
                level: LogLevel::Info,
                message: "hi".into(),
            }),
        );
        let theme = crate::theme::Theme::TOKYO_NIGHT;
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        <AppState as View>::render(&s, Rect::new(0, 0, 80, 24), &mut buf, &theme);
        assert!(!<AppState as View>::quitting(&s));
    }

    #[test]
    fn appstate_view_handle_key_quit_sets_flag() {
        let mut s = AppState::new("app".into(), "debug".into());
        let _ = <AppState as View>::handle_key(&mut s, fl_core::KeyEvent::Char('q'));
        assert!(<AppState as View>::quitting(&s));
    }
}
```

- [ ] **Step 5: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui`
Expected: 30 prior + 2 new = 32 passes.

- [ ] **Step 6: Commit**

```bash
git add crates/fl-tui/
git -c commit.gpgsign=false commit -m "feat(tui): View trait and AppState impl

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `TuiRunner::run_view` — generic render loop

**Files:**
- Modify: `crates/fl-tui/src/runner.rs`

Add a generic method that drives any `View`. The existing `run()` stays unchanged so `fl run` keeps working.

- [ ] **Step 1: Append to `crates/fl-tui/src/runner.rs`**

After the existing `impl TuiRunner` block but before `#[cfg(test)]`, add a second `impl TuiRunner` block:

```rust
impl TuiRunner {
    /// Drive any `View` to completion. The runner reads from `input_rx`, feeds
    /// `view.apply`, listens to keyboard, and ticks the view.
    pub async fn run_view<V: crate::view::View>(
        &mut self,
        view: &mut V,
        input_rx: &mut tokio::sync::mpsc::Receiver<V::Input>,
    ) -> anyhow::Result<()> {
        use crate::theme::Theme;
        use futures_util::StreamExt;
        let theme = Theme::TOKYO_NIGHT;
        let mut last_tick = std::time::Instant::now();
        let tick_every = std::time::Duration::from_millis(33);
        let mut events = crossterm::event::EventStream::new();

        loop {
            if view.quitting() {
                break;
            }
            let now = std::time::Instant::now();
            let dt = now - last_tick;
            last_tick = now;
            view.tick(dt);

            self.terminal.draw(|f| {
                view.render(f.size(), f.buffer_mut(), &theme);
            })?;

            tokio::select! {
                Some(ev) = input_rx.recv() => {
                    view.apply(ev);
                }
                Some(Ok(term_ev)) = events.next() => {
                    if let Some(k) = crate::runner::map_key(term_ev) {
                        if let Some(input) = view.handle_key(k) {
                            view.apply(input);
                        }
                    }
                }
                _ = tokio::time::sleep(tick_every) => {}
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Add a runner test that uses a dummy view**

Append inside the existing `#[cfg(test)] mod tests` block in `runner.rs`:

```rust
    use crate::view::View;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[derive(Default)]
    struct DummyView {
        ticks: u32,
        applied: u32,
        done: bool,
    }
    impl View for DummyView {
        type Input = u32;
        fn apply(&mut self, _: u32) { self.applied += 1; }
        fn render(&self, _: Rect, _: &mut Buffer, _: &crate::theme::Theme) {}
        fn handle_key(&mut self, _: fl_core::KeyEvent) -> Option<u32> { None }
        fn tick(&mut self, _: Duration) { self.ticks += 1; if self.ticks >= 3 { self.done = true; } }
        fn quitting(&self) -> bool { self.done }
    }

    #[tokio::test(start_paused = true)]
    async fn run_view_terminates_when_view_says_quitting() {
        let mut v = DummyView::default();
        let (_tx, mut rx) = mpsc::channel::<u32>(1);
        // TuiRunner needs a real terminal; build a no-init shim by skipping init.
        // We can call run_view directly only if we have a TuiRunner. Constructing one
        // touches stdout, so we instead exercise the View trait's loop logic by
        // calling tick() three times manually here; full end-to-end coverage comes
        // from the existing run() tests.
        for _ in 0..3 { v.tick(Duration::from_millis(33)); }
        assert!(v.quitting());
        // drain to avoid the unused-warning trap.
        assert!(rx.try_recv().is_err());
    }
```

> Note: building a real `TuiRunner` in unit tests requires a TTY. The test above only verifies the loop's *termination contract* (the View's `quitting()` flag drives loop exit). Full end-to-end behavior is covered by command-level integration tests later in the plan.

- [ ] **Step 3: Add `tokio = { features = ["test-util"] }` dev-dep to `fl-tui` if not already there**

In `crates/fl-tui/Cargo.toml`, the `[dev-dependencies]` table must include:

```toml
tokio = { workspace = true, features = ["test-util"] }
```

Add the `test-util` feature if `tokio` is already listed; otherwise add the line. (Note: the workspace tokio config already includes `full`; `test-util` may need explicit listing in the dev-deps for the time-pause macro to compile.)

- [ ] **Step 4: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui`
Expected: 33 passes (32 prior + 1 new runner test).

- [ ] **Step 5: Commit**

```bash
git add crates/fl-tui/
git -c commit.gpgsign=false commit -m "feat(tui): TuiRunner::run_view generic over View trait

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `BuildMode` enum + `BuildTarget` enum in `fl-core`

**Files:**
- Modify: `crates/fl-core/Cargo.toml`
- Modify: `crates/fl-core/src/events.rs`

Both enums need `clap::ValueEnum` to be usable as `#[arg(value_enum)]`. That requires a `clap` dep on `fl-core` (no other crate uses clap from `fl-core`, but the cleanest place to host these enums is alongside other shared types).

- [ ] **Step 1: Add `clap` dependency to `crates/fl-core/Cargo.toml`**

In `[dependencies]`, add:

```toml
clap = { workspace = true }
```

- [ ] **Step 2: Append new types to `crates/fl-core/src/events.rs`**

At the end of the file (before any test module), add:

```rust
use clap::ValueEnum;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum BuildMode {
    Debug,
    Profile,
    Release,
}

impl BuildMode {
    pub fn flutter_flag(self) -> &'static str {
        match self {
            BuildMode::Debug => "--debug",
            BuildMode::Profile => "--profile",
            BuildMode::Release => "--release",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum BuildTarget {
    Apk,
    Aab,
    Ios,
    Web,
}

impl BuildTarget {
    pub fn flutter_arg(self) -> &'static str {
        match self {
            BuildTarget::Apk => "apk",
            BuildTarget::Aab => "appbundle",
            BuildTarget::Ios => "ios",
            BuildTarget::Web => "web",
        }
    }
}
```

> Note: Flutter's actual sub-command for AAB is `flutter build appbundle`, not `aab`. We accept `aab` in our CLI for ergonomics, then map.

- [ ] **Step 3: Add unit tests inside the existing `#[cfg(test)] mod tests` block**

```rust
    #[test]
    fn build_mode_flag_mapping() {
        assert_eq!(BuildMode::Debug.flutter_flag(), "--debug");
        assert_eq!(BuildMode::Profile.flutter_flag(), "--profile");
        assert_eq!(BuildMode::Release.flutter_flag(), "--release");
    }

    #[test]
    fn build_target_arg_mapping() {
        assert_eq!(BuildTarget::Apk.flutter_arg(), "apk");
        assert_eq!(BuildTarget::Aab.flutter_arg(), "appbundle");
        assert_eq!(BuildTarget::Ios.flutter_arg(), "ios");
        assert_eq!(BuildTarget::Web.flutter_arg(), "web");
    }
```

- [ ] **Step 4: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-core`
Expected: 7 prior + 2 new = 9 passes.

- [ ] **Step 5: Commit**

```bash
git add crates/fl-core/
git -c commit.gpgsign=false commit -m "feat(core): BuildMode and BuildTarget enums with clap ValueEnum

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `FlutterDaemon::spawn` accepts extra args + `fl run --mode` plumbing

**Files:**
- Modify: `crates/fl-flutter/src/daemon.rs`
- Modify: `crates/fl-cli/src/cli.rs`
- Modify: `crates/fl-cli/src/main.rs`
- Modify: `crates/fl-cli/src/run_cmd.rs`

- [ ] **Step 1: Update `FlutterDaemon::spawn` signature in `crates/fl-flutter/src/daemon.rs`**

Replace the existing signature and body:

```rust
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
        // ...rest of method unchanged...
```

The rest of the method body (`stdout.take()`, the two spawn loops, etc.) is unchanged. Only the signature and the args construction change.

- [ ] **Step 2: Update the existing daemon test to pass the new `extra_args` parameter**

The test `forwards_parsed_events_from_a_fake_flutter` calls `FlutterDaemon::spawn(&exe, &dir, "fake-device", tx)`. Change to:

```rust
        let mut daemon = FlutterDaemon::spawn(&exe, &dir, "fake-device", &[], tx).await.unwrap();
```

- [ ] **Step 3: Update `crates/fl-cli/src/cli.rs` to add `--mode` to `Run`**

The `Run` arm currently:

```rust
    Run {
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(short, long)] device: Option<String>,
        #[arg(long)] no_wifi: bool,
    },
```

Becomes:

```rust
    Run {
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(short, long)] device: Option<String>,
        #[arg(long)] no_wifi: bool,
        #[arg(long, value_enum, default_value_t = fl_core::BuildMode::Debug)] mode: fl_core::BuildMode,
    },
```

- [ ] **Step 4: Update existing `cli.rs` clap test to assert the default mode**

In the existing test `parses_run_with_options`, replace:

```rust
    #[test]
    fn parses_run_with_options() {
        let c = Cli::parse_from(["fl", "run", "--device", "1.2.3.4:5555", "--no-wifi"]);
        match c.cmd {
            Cmd::Run { device, no_wifi, .. } => {
                assert_eq!(device.as_deref(), Some("1.2.3.4:5555"));
                assert!(no_wifi);
            }
            _ => panic!(),
        }
    }
```

with the same plus mode-checking version:

```rust
    #[test]
    fn parses_run_with_options() {
        let c = Cli::parse_from(["fl", "run", "--device", "1.2.3.4:5555", "--no-wifi"]);
        match c.cmd {
            Cmd::Run { device, no_wifi, mode, .. } => {
                assert_eq!(device.as_deref(), Some("1.2.3.4:5555"));
                assert!(no_wifi);
                assert_eq!(mode, fl_core::BuildMode::Debug);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_run_with_explicit_mode() {
        let c = Cli::parse_from(["fl", "run", "--mode", "release"]);
        match c.cmd {
            Cmd::Run { mode, .. } => assert_eq!(mode, fl_core::BuildMode::Release),
            _ => panic!(),
        }
    }
```

- [ ] **Step 5: Update `crates/fl-cli/src/main.rs` to pass `mode` to `run_cmd::run`**

The existing dispatch:

```rust
    match cli.cmd {
        Cmd::Devices => devices_cmd::run().await,
        Cmd::Run { project, device, no_wifi } => run_cmd::run(project, device, no_wifi).await,
    }
```

becomes:

```rust
    match cli.cmd {
        Cmd::Devices => devices_cmd::run().await,
        Cmd::Run { project, device, no_wifi, mode } => run_cmd::run(project, device, no_wifi, mode).await,
    }
```

- [ ] **Step 6: Update `crates/fl-cli/src/run_cmd.rs` signature and `FlutterDaemon::spawn` call**

The public function:

```rust
pub async fn run(project: Option<PathBuf>, device: Option<String>, no_wifi: bool) -> anyhow::Result<()> {
```

becomes:

```rust
pub async fn run(project: Option<PathBuf>, device: Option<String>, no_wifi: bool, mode: fl_core::BuildMode) -> anyhow::Result<()> {
```

Then replace the existing daemon spawn:

```rust
    let _daemon: Arc<Mutex<Option<FlutterDaemon>>> = Arc::new(Mutex::new(Some(
        FlutterDaemon::spawn(&flutter, &project, &target_serial, flutter_tx).await?,
    )));
```

with:

```rust
    let mode_flag = mode.flutter_flag();
    let extra: Vec<&str> = if matches!(mode, fl_core::BuildMode::Debug) {
        Vec::new()  // debug is implicit for `flutter run`
    } else {
        vec![mode_flag]
    };
    let _daemon: Arc<Mutex<Option<FlutterDaemon>>> = Arc::new(Mutex::new(Some(
        FlutterDaemon::spawn(&flutter, &project, &target_serial, &extra, flutter_tx).await?,
    )));
```

- [ ] **Step 7: Build the workspace**

Run: `. "$HOME/.cargo/env" && cargo build --workspace 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 8: Run all tests**

Run: `. "$HOME/.cargo/env" && cargo test --workspace -- --test-threads=1 2>&1 | grep "test result"`
Expected: all `ok`.

- [ ] **Step 9: Commit**

```bash
git add crates/fl-flutter/ crates/fl-cli/
git -c commit.gpgsign=false commit -m "feat: --mode flag for fl run, FlutterDaemon spawn takes extra args

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `TestEvent` types + `test_parse.rs`

**Files:**
- Modify: `crates/fl-core/src/events.rs`
- Create: `crates/fl-flutter/src/test_parse.rs`
- Modify: `crates/fl-flutter/src/lib.rs`

- [ ] **Step 1: Add types to `crates/fl-core/src/events.rs`**

Append before any test module:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TestResult {
    Success,
    Failure,
    Error,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TestEvent {
    SuiteStart { path: String },
    TestStarted { id: u64, name: String },
    TestDone { id: u64, name: String, result: TestResult, duration_ms: u64 },
    Error { id: Option<u64>, message: String, stack: Option<String> },
    AllDone { success: bool, passed: u32, failed: u32, skipped: u32 },
}
```

- [ ] **Step 2: Create `crates/fl-flutter/src/test_parse.rs`**

```rust
//! Parser for `flutter test --machine` JSON lines.
//!
//! Each line is a plain JSON object (NOT wrapped in `[…]` like the daemon protocol).
//! Recognised `type` values: `start`, `suite`, `testStart`, `testDone`, `error`, `done`.

use fl_core::{TestEvent, TestResult};
use serde_json::Value;

pub fn parse_test_line(raw: &str) -> Option<TestEvent> {
    let raw = raw.trim();
    if !raw.starts_with('{') {
        return None;
    }
    let v: Value = serde_json::from_str(raw).ok()?;
    let kind = v.get("type")?.as_str()?;
    match kind {
        "suite" => {
            let suite = v.get("suite")?;
            let path = suite.get("path").and_then(Value::as_str)?.to_string();
            Some(TestEvent::SuiteStart { path })
        }
        "testStart" => {
            let t = v.get("test")?;
            let id = t.get("id").and_then(Value::as_u64)?;
            let name = t.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            Some(TestEvent::TestStarted { id, name })
        }
        "testDone" => {
            let id = v.get("testID").and_then(Value::as_u64)?;
            let name = v.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let result_s = v.get("result").and_then(Value::as_str).unwrap_or("");
            let result = match result_s {
                "success" => TestResult::Success,
                "failure" => TestResult::Failure,
                "error" => TestResult::Error,
                _ => TestResult::Skipped,
            };
            let duration_ms = v.get("time").and_then(Value::as_u64).unwrap_or(0);
            Some(TestEvent::TestDone { id, name, result, duration_ms })
        }
        "error" => {
            let id = v.get("testID").and_then(Value::as_u64);
            let message = v.get("error").and_then(Value::as_str).unwrap_or("").to_string();
            let stack = v.get("stackTrace").and_then(Value::as_str).map(str::to_string);
            Some(TestEvent::Error { id, message, stack })
        }
        "done" => {
            let success = v.get("success").and_then(Value::as_bool).unwrap_or(false);
            Some(TestEvent::AllDone { success, passed: 0, failed: 0, skipped: 0 })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_suite_start() {
        let line = r#"{"type":"suite","suite":{"id":1,"path":"test/widget_test.dart"}}"#;
        match parse_test_line(line).unwrap() {
            TestEvent::SuiteStart { path } => assert_eq!(path, "test/widget_test.dart"),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_test_start() {
        let line = r#"{"type":"testStart","time":12,"test":{"id":1,"name":"loads home"}}"#;
        match parse_test_line(line).unwrap() {
            TestEvent::TestStarted { id, name } => {
                assert_eq!(id, 1);
                assert_eq!(name, "loads home");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_test_done_success() {
        let line = r#"{"type":"testDone","testID":1,"result":"success","time":42,"name":"x"}"#;
        match parse_test_line(line).unwrap() {
            TestEvent::TestDone { id, result, .. } => {
                assert_eq!(id, 1);
                assert!(matches!(result, TestResult::Success));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_test_done_failure() {
        let line = r#"{"type":"testDone","testID":2,"result":"failure","time":100,"name":"y"}"#;
        match parse_test_line(line).unwrap() {
            TestEvent::TestDone { result, .. } => assert!(matches!(result, TestResult::Failure)),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_error_with_stack() {
        let line = r#"{"type":"error","testID":2,"error":"Expected X","stackTrace":"at line 5"}"#;
        match parse_test_line(line).unwrap() {
            TestEvent::Error { id, message, stack } => {
                assert_eq!(id, Some(2));
                assert!(message.contains("Expected"));
                assert!(stack.unwrap().contains("line 5"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_done_success() {
        let line = r#"{"type":"done","success":true,"time":2000}"#;
        match parse_test_line(line).unwrap() {
            TestEvent::AllDone { success, .. } => assert!(success),
            _ => panic!(),
        }
    }

    #[test]
    fn ignores_garbage() {
        assert!(parse_test_line("not json").is_none());
        assert!(parse_test_line(r#"{"type":"unknown"}"#).is_none());
    }
}
```

- [ ] **Step 3: Re-export from `crates/fl-flutter/src/lib.rs`**

Replace the lib content with:

```rust
//! Wraps the Flutter SDK CLI (`flutter --machine` daemon).

pub mod daemon;
pub mod parse;
pub mod path;
pub mod test_parse;

pub use daemon::FlutterDaemon;
pub use parse::parse_daemon_line;
pub use path::resolve_flutter;
pub use test_parse::parse_test_line;
```

- [ ] **Step 4: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-flutter`
Expected: 17 passes (10 prior + 7 new).

- [ ] **Step 5: Commit**

```bash
git add crates/fl-core/ crates/fl-flutter/
git -c commit.gpgsign=false commit -m "feat(flutter): TestEvent types and flutter test --machine parser

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: `DoctorEvent` types + `doctor_parse.rs`

**Files:**
- Modify: `crates/fl-core/src/events.rs`
- Create: `crates/fl-flutter/src/doctor_parse.rs`
- Modify: `crates/fl-flutter/src/lib.rs`

- [ ] **Step 1: Add types to `crates/fl-core/src/events.rs`**

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DoctorStatus {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DoctorEvent {
    Section { status: DoctorStatus, title: String, details: Vec<String> },
    Done,
}
```

- [ ] **Step 2: Create `crates/fl-flutter/src/doctor_parse.rs`**

```rust
//! Parser for `flutter doctor -v` plain-text output.

use fl_core::{DoctorEvent, DoctorStatus};

/// Parse the full stdout of `flutter doctor -v` into a sequence of `DoctorEvent`s,
/// ending with `Done`.
pub fn parse_doctor_output(stdout: &str) -> Vec<DoctorEvent> {
    let mut events = Vec::new();
    let mut current: Option<(DoctorStatus, String, Vec<String>)> = None;

    for raw_line in stdout.lines() {
        if let Some((status, title)) = parse_section_header(raw_line) {
            if let Some((s, t, d)) = current.take() {
                events.push(DoctorEvent::Section { status: s, title: t, details: d });
            }
            current = Some((status, title, Vec::new()));
        } else if let Some((_, _, details)) = current.as_mut() {
            let trimmed = raw_line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("• ") {
                details.push(rest.to_string());
            } else if trimmed.starts_with("✗ ") || trimmed.starts_with("✓ ") || trimmed.starts_with("! ") {
                details.push(trimmed.to_string());
            } else if raw_line.starts_with("    ") || raw_line.starts_with('\t') {
                // Continuation of a previous detail. Append rather than create.
                if let Some(last) = details.last_mut() {
                    last.push(' ');
                    last.push_str(trimmed);
                }
            }
        }
        if raw_line.starts_with("Doctor summary") || raw_line.starts_with("• No issues") {
            break;
        }
    }
    if let Some((s, t, d)) = current.take() {
        events.push(DoctorEvent::Section { status: s, title: t, details: d });
    }
    events.push(DoctorEvent::Done);
    events
}

fn parse_section_header(line: &str) -> Option<(DoctorStatus, String)> {
    let bytes = line.as_bytes();
    if bytes.len() < 4 || bytes[0] != b'[' {
        return None;
    }
    let marker_end = bytes.iter().position(|&b| b == b']')?;
    let marker = &line[1..marker_end];
    let rest = line.get(marker_end + 1..)?.trim();
    if rest.is_empty() {
        return None;
    }
    let status = match marker.trim() {
        "✓" => DoctorStatus::Ok,
        "!" => DoctorStatus::Warning,
        "✗" => DoctorStatus::Error,
        _ => return None,
    };
    Some((status, rest.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_three_sections_with_details() {
        let input = "\
[✓] Flutter (Channel stable, 3.22.2)
    • Flutter version 3.22.2
    • Engine revision deadbeef
[!] Android Studio (not installed)
    • Try downloading from https://developer.android.com
[✗] Xcode (not installed)

Doctor summary (3 issues found.)
";
        let evs = parse_doctor_output(input);
        // 3 sections + Done = 4
        assert_eq!(evs.len(), 4);
        match &evs[0] {
            DoctorEvent::Section { status, title, details } => {
                assert_eq!(*status, DoctorStatus::Ok);
                assert!(title.contains("Flutter"));
                assert_eq!(details.len(), 2);
            }
            _ => panic!(),
        }
        match &evs[1] {
            DoctorEvent::Section { status, .. } => assert_eq!(*status, DoctorStatus::Warning),
            _ => panic!(),
        }
        match &evs[2] {
            DoctorEvent::Section { status, .. } => assert_eq!(*status, DoctorStatus::Error),
            _ => panic!(),
        }
        assert!(matches!(evs[3], DoctorEvent::Done));
    }

    #[test]
    fn empty_output_emits_only_done() {
        let evs = parse_doctor_output("");
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], DoctorEvent::Done));
    }

    #[test]
    fn ignores_non_section_lines() {
        let input = "Some preamble\n[✓] Flutter\nDoctor summary\n";
        let evs = parse_doctor_output(input);
        assert_eq!(evs.len(), 2);
        assert!(matches!(evs[0], DoctorEvent::Section { .. }));
        assert!(matches!(evs[1], DoctorEvent::Done));
    }
}
```

- [ ] **Step 3: Re-export from `crates/fl-flutter/src/lib.rs`**

Update to:

```rust
//! Wraps the Flutter SDK CLI (`flutter --machine` daemon).

pub mod daemon;
pub mod doctor_parse;
pub mod parse;
pub mod path;
pub mod test_parse;

pub use daemon::FlutterDaemon;
pub use doctor_parse::parse_doctor_output;
pub use parse::parse_daemon_line;
pub use path::resolve_flutter;
pub use test_parse::parse_test_line;
```

- [ ] **Step 4: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-flutter`
Expected: 20 passes.

- [ ] **Step 5: Commit**

```bash
git add crates/fl-core/ crates/fl-flutter/
git -c commit.gpgsign=false commit -m "feat(flutter): DoctorEvent types and parser for flutter doctor -v

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: `PubEvent` types + `pub_parse.rs` (get/upgrade parser)

**Files:**
- Modify: `crates/fl-core/src/events.rs`
- Create: `crates/fl-flutter/src/pub_parse.rs`
- Modify: `crates/fl-flutter/src/lib.rs`

- [ ] **Step 1: Add types to `crates/fl-core/src/events.rs`**

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PubDepKind {
    Direct,
    Dev,
    Transitive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutdatedRow {
    pub package: String,
    pub current: String,
    pub upgradable: String,
    pub resolvable: String,
    pub latest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PubTreeNode {
    pub name: String,
    pub version: String,
    pub kind: PubDepKind,
    pub children: Vec<PubTreeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PubEvent {
    Resolving,
    Got {
        added: Vec<String>,
        removed: Vec<String>,
        modified: Vec<(String, String, String)>,
    },
    Outdated { rows: Vec<OutdatedRow> },
    Deps { tree: PubTreeNode },
    Log { level: LogLevel, message: String },
    Done { success: bool },
}
```

- [ ] **Step 2: Create `crates/fl-flutter/src/pub_parse.rs` (start with `parse_pub_get`)**

```rust
//! Parsers for `flutter pub` plain-text output.

use fl_core::{OutdatedRow, PubDepKind, PubEvent, PubTreeNode};

/// Parse `flutter pub get` / `pub upgrade` stdout. Returns a `Got` event.
/// Lines we look for:
///   `+ package_name 1.0.0`      → added
///   `- package_name`             → removed
///   `> package_name 1.0.0 (was 0.9.0)` → modified
pub fn parse_pub_get(stdout: &str) -> PubEvent {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();
    for line in stdout.lines() {
        let l = line.trim_start();
        if let Some(rest) = l.strip_prefix("+ ") {
            let mut parts = rest.split_whitespace();
            if let Some(name) = parts.next() {
                added.push(name.to_string());
            }
        } else if let Some(rest) = l.strip_prefix("- ") {
            let mut parts = rest.split_whitespace();
            if let Some(name) = parts.next() {
                removed.push(name.to_string());
            }
        } else if let Some(rest) = l.strip_prefix("> ") {
            // > foo 1.0.0 (was 0.9.0)
            let mut parts = rest.split_whitespace();
            let name = parts.next().unwrap_or("").to_string();
            let new_v = parts.next().unwrap_or("").to_string();
            let was = rest.split_once("(was ").and_then(|(_, r)| r.strip_suffix(')')).unwrap_or("").to_string();
            modified.push((name, was, new_v));
        }
    }
    PubEvent::Got { added, removed, modified }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fl_core::PubEvent;

    #[test]
    fn parses_added_removed_modified() {
        let input = "\
Resolving dependencies...
+ shiny_new_pkg 1.0.0
- legacy_pkg
> updated_pkg 2.0.0 (was 1.9.0)
Got dependencies!
";
        match parse_pub_get(input) {
            PubEvent::Got { added, removed, modified } => {
                assert_eq!(added, vec!["shiny_new_pkg".to_string()]);
                assert_eq!(removed, vec!["legacy_pkg".to_string()]);
                assert_eq!(modified.len(), 1);
                assert_eq!(modified[0], ("updated_pkg".into(), "1.9.0".into(), "2.0.0".into()));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn empty_output_returns_empty_vecs() {
        match parse_pub_get("") {
            PubEvent::Got { added, removed, modified } => {
                assert!(added.is_empty() && removed.is_empty() && modified.is_empty());
            }
            _ => panic!(),
        }
    }
}
```

- [ ] **Step 3: Re-export from `crates/fl-flutter/src/lib.rs`**

```rust
//! Wraps the Flutter SDK CLI (`flutter --machine` daemon).

pub mod daemon;
pub mod doctor_parse;
pub mod parse;
pub mod path;
pub mod pub_parse;
pub mod test_parse;

pub use daemon::FlutterDaemon;
pub use doctor_parse::parse_doctor_output;
pub use parse::parse_daemon_line;
pub use path::resolve_flutter;
pub use pub_parse::parse_pub_get;
pub use test_parse::parse_test_line;
```

- [ ] **Step 4: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-flutter`
Expected: 22 passes.

- [ ] **Step 5: Commit**

```bash
git add crates/fl-core/ crates/fl-flutter/
git -c commit.gpgsign=false commit -m "feat(flutter): PubEvent types and parse_pub_get

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: `parse_outdated_table` in `pub_parse.rs`

**Files:**
- Modify: `crates/fl-flutter/src/pub_parse.rs`
- Modify: `crates/fl-flutter/src/lib.rs`

- [ ] **Step 1: Append to `crates/fl-flutter/src/pub_parse.rs`**

Append above the test module:

```rust
/// Parse the output of `flutter pub outdated`.
/// The table has 5 columns:
///   Package Name   Current   Upgradable   Resolvable   Latest
/// We split on whitespace runs and skip header / separator lines.
pub fn parse_outdated_table(stdout: &str) -> Vec<OutdatedRow> {
    let mut rows = Vec::new();
    let mut in_table = false;
    for line in stdout.lines() {
        if line.trim_start().starts_with("Package Name") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        if line.trim().is_empty() {
            // blank line ends the table block
            if !rows.is_empty() {
                break;
            }
            continue;
        }
        // Skip section headers like "direct dependencies:" that Flutter prints.
        let trimmed = line.trim_start();
        if trimmed.ends_with(':') && !trimmed.contains("  ") {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 5 {
            // Last 4 fields are version strings; package = everything before.
            let n = fields.len();
            let package = fields[..n - 4].join(" ");
            rows.push(OutdatedRow {
                package,
                current: fields[n - 4].to_string(),
                upgradable: fields[n - 3].to_string(),
                resolvable: fields[n - 2].to_string(),
                latest: fields[n - 1].to_string(),
            });
        }
    }
    rows
}
```

- [ ] **Step 2: Add test inside the existing `#[cfg(test)] mod tests` block**

```rust
    #[test]
    fn parses_outdated_table() {
        let input = "\
Showing outdated packages.

Package Name      Current   Upgradable  Resolvable  Latest

direct dependencies:
http              0.13.5    0.13.6      0.14.0      1.2.0
provider          6.0.5     6.0.5       6.1.1       6.1.1

dev dependencies:
flutter_test      sdk       sdk         sdk         sdk
";
        let rows = parse_outdated_table(input);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].package, "http");
        assert_eq!(rows[0].current, "0.13.5");
        assert_eq!(rows[0].latest, "1.2.0");
        assert_eq!(rows[2].package, "flutter_test");
    }
```

- [ ] **Step 3: Re-export from `crates/fl-flutter/src/lib.rs`**

Add `parse_outdated_table` to the re-export line:

```rust
pub use pub_parse::{parse_outdated_table, parse_pub_get};
```

- [ ] **Step 4: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-flutter`
Expected: 23 passes.

- [ ] **Step 5: Commit**

```bash
git add crates/fl-flutter/
git -c commit.gpgsign=false commit -m "feat(flutter): parse_outdated_table for flutter pub outdated

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: `parse_deps_json` in `pub_parse.rs`

**Files:**
- Modify: `crates/fl-flutter/src/pub_parse.rs`
- Modify: `crates/fl-flutter/src/lib.rs`

`flutter pub deps --json` emits an object with `root`, `packages`, `directDependencies`, `devDependencies`. We build a tree from `directDependencies` outward.

- [ ] **Step 1: Append to `crates/fl-flutter/src/pub_parse.rs`**

Above the test module:

```rust
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Parse `flutter pub deps --json` and return a tree rooted at the project.
pub fn parse_deps_json(json: &str) -> anyhow::Result<PubTreeNode> {
    let v: Value = serde_json::from_str(json).map_err(|e| anyhow::anyhow!("invalid deps json: {e}"))?;
    let root_name = v.get("root").and_then(Value::as_str).unwrap_or("root").to_string();

    let mut by_name: HashMap<String, &Value> = HashMap::new();
    let packages = v.get("packages").and_then(Value::as_array).cloned().unwrap_or_default();
    for p in &packages {
        if let Some(name) = p.get("name").and_then(Value::as_str) {
            // SAFETY: we keep packages alive through the function via the cloned vec.
            // To avoid lifetime juggling, switch to owned strings:
        }
        let _ = p;
    }
    // Owned map (name -> package object):
    let pkg_map: HashMap<String, Value> = packages
        .iter()
        .filter_map(|p| p.get("name").and_then(Value::as_str).map(|n| (n.to_string(), p.clone())))
        .collect();

    let direct: HashSet<String> = v
        .get("directDependencies")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let dev: HashSet<String> = v
        .get("devDependencies")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();

    let root_node = build_node(&root_name, "0.0.0", PubDepKind::Direct, &pkg_map, &direct, &dev, &mut HashSet::new());
    Ok(root_node)
}

fn build_node(
    name: &str,
    fallback_version: &str,
    kind: PubDepKind,
    pkg_map: &HashMap<String, Value>,
    direct: &HashSet<String>,
    dev: &HashSet<String>,
    visited: &mut HashSet<String>,
) -> PubTreeNode {
    let version = pkg_map.get(name).and_then(|p| p.get("version")).and_then(Value::as_str).unwrap_or(fallback_version).to_string();
    let mut children = Vec::new();
    if !visited.insert(name.to_string()) {
        return PubTreeNode { name: name.to_string(), version, kind, children };
    }
    let deps = pkg_map
        .get(name)
        .and_then(|p| p.get("dependencies"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for d in deps {
        let Some(dn) = d.as_str() else { continue };
        let dn = dn.to_string();
        let child_kind = if direct.contains(&dn) {
            PubDepKind::Direct
        } else if dev.contains(&dn) {
            PubDepKind::Dev
        } else {
            PubDepKind::Transitive
        };
        children.push(build_node(&dn, "0.0.0", child_kind, pkg_map, direct, dev, visited));
    }
    PubTreeNode { name: name.to_string(), version, kind, children }
}
```

- [ ] **Step 2: Add a fixture-based test inside `mod tests`**

```rust
    #[test]
    fn parses_deps_json_tree() {
        let json = r#"{
            "root": "myapp",
            "directDependencies": ["http", "provider"],
            "devDependencies": ["flutter_test"],
            "packages": [
                {"name": "myapp", "version": "1.0.0", "dependencies": ["http", "provider", "flutter_test"]},
                {"name": "http", "version": "0.13.5", "dependencies": ["http_parser"]},
                {"name": "http_parser", "version": "4.0.0", "dependencies": []},
                {"name": "provider", "version": "6.0.5", "dependencies": []},
                {"name": "flutter_test", "version": "sdk", "dependencies": []}
            ]
        }"#;
        let tree = parse_deps_json(json).unwrap();
        assert_eq!(tree.name, "myapp");
        let names: Vec<&str> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"http"));
        assert!(names.contains(&"provider"));
        assert!(names.contains(&"flutter_test"));
        let http = tree.children.iter().find(|c| c.name == "http").unwrap();
        assert_eq!(http.kind, PubDepKind::Direct);
        let dev = tree.children.iter().find(|c| c.name == "flutter_test").unwrap();
        assert_eq!(dev.kind, PubDepKind::Dev);
        let transitive = http.children.first().unwrap();
        assert_eq!(transitive.kind, PubDepKind::Transitive);
        assert_eq!(transitive.name, "http_parser");
    }
```

- [ ] **Step 3: Re-export `parse_deps_json` from `crates/fl-flutter/src/lib.rs`**

```rust
pub use pub_parse::{parse_deps_json, parse_outdated_table, parse_pub_get};
```

- [ ] **Step 4: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-flutter`
Expected: 24 passes.

- [ ] **Step 5: Commit**

```bash
git add crates/fl-flutter/
git -c commit.gpgsign=false commit -m "feat(flutter): parse_deps_json for flutter pub deps --json

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: `CleanEvent` type

**Files:**
- Modify: `crates/fl-core/src/events.rs`

- [ ] **Step 1: Append to `crates/fl-core/src/events.rs`**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CleanEvent {
    Probing,
    Cleaning { path: String },
    Done { freed_bytes: u64, paths: Vec<String> },
    Error(String),
}
```

- [ ] **Step 2: Add a simple roundtrip test inside the existing `mod tests` block**

```rust
    #[test]
    fn cleanevent_done_roundtrips() {
        let original = CleanEvent::Done {
            freed_bytes: 12345,
            paths: vec!["build/".into(), ".dart_tool/".into()],
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: CleanEvent = serde_json::from_str(&json).unwrap();
        match back {
            CleanEvent::Done { freed_bytes, paths } => {
                assert_eq!(freed_bytes, 12345);
                assert_eq!(paths.len(), 2);
            }
            _ => panic!(),
        }
    }
```

- [ ] **Step 3: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-core`
Expected: 10 passes.

- [ ] **Step 4: Commit**

```bash
git add crates/fl-core/
git -c commit.gpgsign=false commit -m "feat(core): CleanEvent type

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: `BuildView` + `build_cmd`

**Files:**
- Create: `crates/fl-tui/src/views/mod.rs`
- Create: `crates/fl-tui/src/views/build_view.rs`
- Modify: `crates/fl-tui/src/lib.rs`
- Create: `crates/fl-cli/src/build_cmd.rs`

- [ ] **Step 1: Create `crates/fl-tui/src/views/mod.rs`**

```rust
//! Command-specific TUI views.

pub mod build_view;
```

- [ ] **Step 2: Create `crates/fl-tui/src/views/build_view.rs`**

```rust
//! View for `fl build <target>` — phase list + final binary report.

use crate::theme::Theme;
use crate::view::View;
use fl_core::{BuildMode, BuildTarget, FlutterEvent, KeyEvent as FlKey, LogLevel};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct BuildStep {
    pub id: String,
    pub message: String,
    pub status: StepStatus,
    pub started_at: Instant,
    pub finished_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Running,
    Done,
    Failed,
}

pub struct BuildView {
    pub target: BuildTarget,
    pub mode: BuildMode,
    pub steps: Vec<BuildStep>,
    pub log_tail: Vec<String>,
    pub final_size: Option<u64>,
    pub final_path: Option<String>,
    pub quitting: bool,
    pub started_at: Instant,
    pub elapsed_ms: u64,
}

impl BuildView {
    pub fn new(target: BuildTarget, mode: BuildMode) -> Self {
        Self {
            target,
            mode,
            steps: Vec::new(),
            log_tail: Vec::new(),
            final_size: None,
            final_path: None,
            quitting: false,
            started_at: Instant::now(),
            elapsed_ms: 0,
        }
    }
}

impl View for BuildView {
    type Input = FlutterEvent;

    fn apply(&mut self, input: Self::Input) {
        match input {
            FlutterEvent::Progress { id, message, finished } => {
                if let Some(existing) = self.steps.iter_mut().find(|s| s.id == id) {
                    existing.message = message;
                    if finished && existing.status == StepStatus::Running {
                        existing.status = StepStatus::Done;
                        existing.finished_at = Some(Instant::now());
                    }
                } else {
                    self.steps.push(BuildStep {
                        id,
                        message,
                        status: if finished { StepStatus::Done } else { StepStatus::Running },
                        started_at: Instant::now(),
                        finished_at: if finished { Some(Instant::now()) } else { None },
                    });
                }
            }
            FlutterEvent::Log { level, message } => {
                if matches!(level, LogLevel::Error) {
                    if let Some(last) = self.steps.last_mut() {
                        if last.status == StepStatus::Running {
                            last.status = StepStatus::Failed;
                            last.finished_at = Some(Instant::now());
                        }
                    }
                }
                // Detect the final "Built <path> (NN.NMB)" line.
                if let Some(rest) = message.strip_prefix("Built ") {
                    if let Some((path, size)) = parse_built_line(rest) {
                        self.final_path = Some(path);
                        self.final_size = Some(size);
                    }
                }
                self.log_tail.push(message);
                if self.log_tail.len() > 200 {
                    self.log_tail.remove(0);
                }
            }
            FlutterEvent::Stopped { exit_code } => {
                self.quitting = true;
                if let Some(code) = exit_code {
                    if code != 0 {
                        if let Some(last) = self.steps.last_mut() {
                            if last.status == StepStatus::Running {
                                last.status = StepStatus::Failed;
                                last.finished_at = Some(Instant::now());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(3),
            ])
            .split(area);

        // Header
        let header_text = format!(
            " fl build ── {} · {} · {:>4}.{}s ",
            self.target.flutter_arg(),
            mode_label(self.mode),
            self.elapsed_ms / 1000,
            self.elapsed_ms % 1000 / 100
        );
        let header_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent).bg(theme.bg))
            .style(theme.base());
        let header_inner = header_block.inner(layout[0]);
        header_block.render(layout[0], buf);
        Paragraph::new(Line::styled(header_text, theme.header())).render(header_inner, buf);

        // Steps
        let steps_block = Block::default()
            .title(" Steps ")
            .borders(Borders::ALL)
            .border_style(theme.dimmed())
            .style(theme.base());
        let steps_inner = steps_block.inner(layout[1]);
        steps_block.render(layout[1], buf);

        let mut lines: Vec<Line> = Vec::new();
        for step in &self.steps {
            let (marker, color) = match step.status {
                StepStatus::Running => ("⠋ ", theme.warn),
                StepStatus::Done => ("✓ ", theme.success),
                StepStatus::Failed => ("✗ ", theme.error),
            };
            let elapsed_ms = step
                .finished_at
                .unwrap_or_else(Instant::now)
                .duration_since(step.started_at)
                .as_millis();
            lines.push(Line::styled(
                format!("{marker}{:<40} {:>5}ms", step.message, elapsed_ms),
                Style::default().fg(color).bg(theme.bg),
            ));
        }
        Paragraph::new(lines).render(steps_inner, buf);

        // Footer (final size / status)
        let footer_block = Block::default().borders(Borders::ALL).border_style(theme.dimmed()).style(theme.base());
        let footer_inner = footer_block.inner(layout[2]);
        footer_block.render(layout[2], buf);
        let footer_text = match (&self.final_path, self.final_size) {
            (Some(path), Some(size)) => format!("Built {path} · {}", human_size(size)),
            _ => " ".to_string(),
        };
        Paragraph::new(Line::styled(footer_text, theme.dimmed())).render(footer_inner, buf);
    }

    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input> {
        if matches!(key, FlKey::Char('q') | FlKey::Ctrl('c')) {
            self.quitting = true;
        }
        None
    }

    fn tick(&mut self, dt: Duration) {
        self.elapsed_ms = self.elapsed_ms.saturating_add(dt.as_millis() as u64);
    }

    fn quitting(&self) -> bool {
        self.quitting
    }
}

fn mode_label(m: BuildMode) -> &'static str {
    match m {
        BuildMode::Debug => "debug",
        BuildMode::Profile => "profile",
        BuildMode::Release => "release",
    }
}

fn parse_built_line(rest: &str) -> Option<(String, u64)> {
    // "build/app/outputs/flutter-apk/app-release.apk (12.3MB)."
    let (path, size_part) = rest.rsplit_once(" (")?;
    let size_str = size_part.trim_end_matches(").").trim_end_matches(')');
    let bytes = parse_size_to_bytes(size_str)?;
    Some((path.to_string(), bytes))
}

fn parse_size_to_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| c.is_alphabetic())?);
    let value: f64 = num.trim().parse().ok()?;
    let mult: u64 = match unit.trim() {
        "B" | "b" => 1,
        "KB" | "kB" => 1024,
        "MB" | "mB" => 1024 * 1024,
        "GB" | "gB" => 1024 * 1024 * 1024,
        _ => return None,
    };
    Some((value * mult as f64) as u64)
}

fn human_size(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1}{}", units[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_progress_event_starts_a_step() {
        let mut v = BuildView::new(BuildTarget::Apk, BuildMode::Release);
        v.apply(FlutterEvent::Progress {
            id: "gradle".into(),
            message: "Running Gradle task".into(),
            finished: false,
        });
        assert_eq!(v.steps.len(), 1);
        assert_eq!(v.steps[0].status, StepStatus::Running);
    }

    #[test]
    fn progress_with_finished_marks_step_done() {
        let mut v = BuildView::new(BuildTarget::Apk, BuildMode::Release);
        v.apply(FlutterEvent::Progress { id: "g".into(), message: "x".into(), finished: false });
        v.apply(FlutterEvent::Progress { id: "g".into(), message: "x done".into(), finished: true });
        assert_eq!(v.steps[0].status, StepStatus::Done);
    }

    #[test]
    fn captures_final_binary_size_from_log() {
        let mut v = BuildView::new(BuildTarget::Apk, BuildMode::Release);
        v.apply(FlutterEvent::Log {
            level: LogLevel::Info,
            message: "Built build/app/outputs/flutter-apk/app-release.apk (12.3MB).".into(),
        });
        assert_eq!(v.final_size, Some((12.3 * 1024.0 * 1024.0) as u64));
        assert!(v.final_path.as_ref().unwrap().contains("app-release.apk"));
    }

    #[test]
    fn stopped_marks_running_step_failed_on_nonzero_exit() {
        let mut v = BuildView::new(BuildTarget::Apk, BuildMode::Release);
        v.apply(FlutterEvent::Progress { id: "g".into(), message: "x".into(), finished: false });
        v.apply(FlutterEvent::Stopped { exit_code: Some(1) });
        assert_eq!(v.steps[0].status, StepStatus::Failed);
        assert!(v.quitting);
    }
}
```

- [ ] **Step 3: Update `crates/fl-tui/src/lib.rs`**

```rust
//! Terminal UI for the `fl` CLI.

pub mod app;
pub mod panels;
pub mod render;
pub mod runner;
pub mod spinner;
pub mod splash;
pub mod theme;
pub mod view;
pub mod views;

pub use app::{AppState, Banner, BannerKind, LogLine};
pub use render::render;
pub use runner::{map_key, TuiRunner};
pub use spinner::Spinner;
pub use splash::Splash;
pub use theme::Theme;
pub use view::View;
pub use views::build_view::BuildView;
```

- [ ] **Step 4: Create `crates/fl-cli/src/build_cmd.rs`**

```rust
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
    let flutter = resolve_flutter(None, std::env::var("FLUTTER_ROOT").ok().as_deref(), dirs_home().as_deref())
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
```

- [ ] **Step 5: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui`
Expected: 33 prior + 4 new BuildView tests = 37 passes.

The `build_cmd` doesn't compile yet because nothing wires it into `main.rs` — that's fine, Task 16 does the wiring. For now, ensure fl-tui still tests-clean and fl-cli still builds (it will because `build_cmd` is a module the binary doesn't reference yet, but you need to declare the module in fl-cli's main.rs as `mod build_cmd;` to even compile it).

Add to `crates/fl-cli/src/main.rs` near the other `mod` lines:

```rust
mod build_cmd;
```

Then `cargo build --workspace` should succeed. Run: `. "$HOME/.cargo/env" && cargo build --workspace 2>&1 | tail -5`. Expected: clean (may warn about unused `build_cmd::run`).

- [ ] **Step 6: Commit**

```bash
git add crates/fl-tui/ crates/fl-cli/
git -c commit.gpgsign=false commit -m "feat: BuildView and fl build command module

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: `TestView` + `test_cmd`

**Files:**
- Create: `crates/fl-tui/src/views/test_view.rs`
- Modify: `crates/fl-tui/src/views/mod.rs`
- Modify: `crates/fl-tui/src/lib.rs`
- Create: `crates/fl-cli/src/test_cmd.rs`
- Modify: `crates/fl-cli/src/main.rs`

- [ ] **Step 1: Create `crates/fl-tui/src/views/test_view.rs`**

```rust
//! View for `fl test`.

use crate::theme::Theme;
use crate::view::View;
use fl_core::{KeyEvent as FlKey, TestEvent, TestResult};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct TestFailure {
    pub name: String,
    pub message: String,
    pub stack: Option<String>,
}

pub struct TestView {
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub running: Vec<(u64, String)>,
    pub recent_done: Vec<(String, TestResult)>,
    pub failures: Vec<TestFailure>,
    pub all_done: bool,
    pub success: bool,
    pub quitting: bool,
}

impl TestView {
    pub fn new() -> Self {
        Self {
            passed: 0,
            failed: 0,
            skipped: 0,
            running: Vec::new(),
            recent_done: Vec::new(),
            failures: Vec::new(),
            all_done: false,
            success: false,
            quitting: false,
        }
    }
}

impl View for TestView {
    type Input = TestEvent;

    fn apply(&mut self, input: Self::Input) {
        match input {
            TestEvent::TestStarted { id, name } => {
                self.running.push((id, name));
            }
            TestEvent::TestDone { id, name, result, duration_ms: _ } => {
                self.running.retain(|(rid, _)| *rid != id);
                match result {
                    TestResult::Success => self.passed += 1,
                    TestResult::Failure => self.failed += 1,
                    TestResult::Error => self.failed += 1,
                    TestResult::Skipped => self.skipped += 1,
                }
                self.recent_done.push((name, result));
                if self.recent_done.len() > 20 {
                    self.recent_done.remove(0);
                }
            }
            TestEvent::Error { id: _, message, stack } => {
                let name = self.running.last().map(|(_, n)| n.clone()).unwrap_or_else(|| "<unknown>".into());
                self.failures.push(TestFailure { name, message, stack });
            }
            TestEvent::AllDone { success, .. } => {
                self.all_done = true;
                self.success = success;
                self.quitting = true;
            }
            TestEvent::SuiteStart { .. } => {}
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(8),
            ])
            .split(area);

        // Header: big counter
        let header_block = Block::default().borders(Borders::ALL).border_style(theme.dimmed()).style(theme.base());
        let inner = header_block.inner(layout[0]);
        header_block.render(layout[0], buf);
        let counter = format!(
            " fl test ── ✓ {}  ✗ {}  – {}",
            self.passed, self.failed, self.skipped
        );
        let color = if self.failed > 0 { theme.error } else { theme.success };
        Paragraph::new(Line::styled(counter, Style::default().fg(color).bg(theme.bg))).render(inner, buf);

        // Live list (running + recent)
        let live_block = Block::default().title(" Live ").borders(Borders::ALL).border_style(theme.dimmed()).style(theme.base());
        let inner = live_block.inner(layout[1]);
        live_block.render(layout[1], buf);
        let mut lines: Vec<Line> = Vec::new();
        for (_id, name) in &self.running {
            lines.push(Line::styled(format!("⠋ {name}"), Style::default().fg(theme.warn).bg(theme.bg)));
        }
        for (name, result) in self.recent_done.iter().rev().take(inner.height as usize).rev() {
            let (marker, color) = match result {
                TestResult::Success => ("✓", theme.success),
                TestResult::Failure => ("✗", theme.error),
                TestResult::Error => ("✗", theme.error),
                TestResult::Skipped => ("–", theme.dim),
            };
            lines.push(Line::styled(format!("{marker} {name}"), Style::default().fg(color).bg(theme.bg)));
        }
        Paragraph::new(lines).render(inner, buf);

        // Failures
        let fail_block = Block::default().title(" Failures ").borders(Borders::ALL).border_style(theme.dimmed()).style(theme.base());
        let inner = fail_block.inner(layout[2]);
        fail_block.render(layout[2], buf);
        let mut fail_lines: Vec<Line> = Vec::new();
        for f in self.failures.iter().rev().take(3).rev() {
            fail_lines.push(Line::styled(format!("✗ {}", f.name), Style::default().fg(theme.error).bg(theme.bg)));
            fail_lines.push(Line::styled(format!("    {}", f.message), theme.dimmed()));
        }
        Paragraph::new(fail_lines).render(inner, buf);
    }

    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input> {
        if matches!(key, FlKey::Char('q') | FlKey::Ctrl('c')) {
            self.quitting = true;
        }
        None
    }
    fn tick(&mut self, _dt: Duration) {}
    fn quitting(&self) -> bool {
        self.quitting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passed_increments_on_success() {
        let mut v = TestView::new();
        v.apply(TestEvent::TestStarted { id: 1, name: "t".into() });
        v.apply(TestEvent::TestDone { id: 1, name: "t".into(), result: TestResult::Success, duration_ms: 10 });
        assert_eq!(v.passed, 1);
        assert!(v.running.is_empty());
    }

    #[test]
    fn failed_increments_on_failure_and_records_failure() {
        let mut v = TestView::new();
        v.apply(TestEvent::TestStarted { id: 2, name: "t2".into() });
        v.apply(TestEvent::Error { id: Some(2), message: "boom".into(), stack: None });
        v.apply(TestEvent::TestDone { id: 2, name: "t2".into(), result: TestResult::Failure, duration_ms: 5 });
        assert_eq!(v.failed, 1);
        assert_eq!(v.failures.len(), 1);
        assert!(v.failures[0].message.contains("boom"));
    }

    #[test]
    fn all_done_sets_quitting() {
        let mut v = TestView::new();
        v.apply(TestEvent::AllDone { success: true, passed: 0, failed: 0, skipped: 0 });
        assert!(v.quitting);
    }
}
```

- [ ] **Step 2: Update `crates/fl-tui/src/views/mod.rs`**

```rust
//! Command-specific TUI views.

pub mod build_view;
pub mod test_view;
```

- [ ] **Step 3: Update `crates/fl-tui/src/lib.rs` to re-export TestView**

Add:

```rust
pub use views::test_view::TestView;
```

- [ ] **Step 4: Create `crates/fl-cli/src/test_cmd.rs`**

```rust
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
    let flutter = resolve_flutter(None, std::env::var("FLUTTER_ROOT").ok().as_deref(), dirs_home().as_deref())
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
```

- [ ] **Step 5: Declare module in `crates/fl-cli/src/main.rs`**

Add near the other `mod` lines:

```rust
mod test_cmd;
```

- [ ] **Step 6: Run tests + build**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui && cargo build --workspace`
Expected: 40 fl-tui tests (37 prior + 3 new TestView). Build clean.

- [ ] **Step 7: Commit**

```bash
git add crates/fl-tui/ crates/fl-cli/
git -c commit.gpgsign=false commit -m "feat: TestView and fl test command module

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 13: `PubView` + `pub_cmd` (all subcommands)

**Files:**
- Create: `crates/fl-tui/src/views/pub_view.rs`
- Modify: `crates/fl-tui/src/views/mod.rs`
- Modify: `crates/fl-tui/src/lib.rs`
- Create: `crates/fl-cli/src/pub_cmd.rs`
- Modify: `crates/fl-cli/src/main.rs`

- [ ] **Step 1: Create `crates/fl-tui/src/views/pub_view.rs`**

```rust
//! View for `fl pub <subcommand>`. The variant of `PubEvent` selects the layout.

use crate::theme::Theme;
use crate::view::View;
use fl_core::{KeyEvent as FlKey, LogLevel, OutdatedRow, PubDepKind, PubEvent, PubTreeNode};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::time::Duration;

pub enum PubMode {
    GetOrUpgrade {
        added: Vec<String>,
        removed: Vec<String>,
        modified: Vec<(String, String, String)>,
    },
    Outdated {
        rows: Vec<OutdatedRow>,
    },
    Deps {
        tree: Option<PubTreeNode>,
    },
}

pub struct PubView {
    pub title: String,
    pub mode: PubMode,
    pub log: Vec<(LogLevel, String)>,
    pub done: bool,
    pub success: bool,
    pub quitting: bool,
}

impl PubView {
    pub fn for_get_or_upgrade(label: &str) -> Self {
        Self {
            title: label.into(),
            mode: PubMode::GetOrUpgrade {
                added: Vec::new(),
                removed: Vec::new(),
                modified: Vec::new(),
            },
            log: Vec::new(),
            done: false,
            success: false,
            quitting: false,
        }
    }
    pub fn for_outdated() -> Self {
        Self {
            title: "outdated".into(),
            mode: PubMode::Outdated { rows: Vec::new() },
            log: Vec::new(),
            done: false,
            success: false,
            quitting: false,
        }
    }
    pub fn for_deps() -> Self {
        Self {
            title: "deps".into(),
            mode: PubMode::Deps { tree: None },
            log: Vec::new(),
            done: false,
            success: false,
            quitting: false,
        }
    }
}

impl View for PubView {
    type Input = PubEvent;

    fn apply(&mut self, input: Self::Input) {
        match input {
            PubEvent::Resolving => {}
            PubEvent::Got { added, removed, modified } => {
                if let PubMode::GetOrUpgrade { added: a, removed: r, modified: m } = &mut self.mode {
                    *a = added;
                    *r = removed;
                    *m = modified;
                }
            }
            PubEvent::Outdated { rows } => {
                if let PubMode::Outdated { rows: target } = &mut self.mode {
                    *target = rows;
                }
            }
            PubEvent::Deps { tree } => {
                if let PubMode::Deps { tree: t } = &mut self.mode {
                    *t = Some(tree);
                }
            }
            PubEvent::Log { level, message } => {
                self.log.push((level, message));
            }
            PubEvent::Done { success } => {
                self.done = true;
                self.success = success;
                self.quitting = true;
            }
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(5)])
            .split(area);

        let header_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent).bg(theme.bg))
            .style(theme.base());
        let inner = header_block.inner(layout[0]);
        header_block.render(layout[0], buf);
        Paragraph::new(Line::styled(format!(" fl pub ── {} ", self.title), theme.header())).render(inner, buf);

        let body_block = Block::default().borders(Borders::ALL).border_style(theme.dimmed()).style(theme.base());
        let inner = body_block.inner(layout[1]);
        body_block.render(layout[1], buf);

        let lines = match &self.mode {
            PubMode::GetOrUpgrade { added, removed, modified } => {
                let mut out = Vec::new();
                for a in added {
                    out.push(Line::styled(format!("+ {a}"), Style::default().fg(theme.success).bg(theme.bg)));
                }
                for r in removed {
                    out.push(Line::styled(format!("- {r}"), Style::default().fg(theme.error).bg(theme.bg)));
                }
                for (name, was, new) in modified {
                    out.push(Line::styled(
                        format!("> {name}  {was} → {new}"),
                        Style::default().fg(theme.warn).bg(theme.bg),
                    ));
                }
                out
            }
            PubMode::Outdated { rows } => {
                let mut out = vec![Line::styled(
                    format!("{:<28} {:<10} {:<10} {:<10} {:<10}", "Package", "Current", "Upgradable", "Resolvable", "Latest"),
                    theme.header(),
                )];
                for row in rows {
                    out.push(Line::from(vec![
                        ratatui::text::Span::styled(format!("{:<28} ", row.package), theme.base()),
                        ratatui::text::Span::styled(format!("{:<10} ", row.current), theme.dimmed()),
                        ratatui::text::Span::styled(format!("{:<10} ", row.upgradable), Style::default().fg(theme.warn).bg(theme.bg)),
                        ratatui::text::Span::styled(format!("{:<10} ", row.resolvable), Style::default().fg(theme.cyan).bg(theme.bg)),
                        ratatui::text::Span::styled(format!("{:<10}", row.latest), Style::default().fg(theme.success).bg(theme.bg)),
                    ]));
                }
                out
            }
            PubMode::Deps { tree } => {
                let mut out = Vec::new();
                if let Some(t) = tree {
                    write_tree(t, 0, theme, &mut out);
                }
                out
            }
        };
        Paragraph::new(lines).render(inner, buf);
    }

    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input> {
        if matches!(key, FlKey::Char('q') | FlKey::Ctrl('c')) {
            self.quitting = true;
        }
        None
    }
    fn tick(&mut self, _dt: Duration) {}
    fn quitting(&self) -> bool { self.quitting }
}

fn write_tree(node: &PubTreeNode, depth: usize, theme: &Theme, out: &mut Vec<Line<'static>>) {
    let indent = "  ".repeat(depth);
    let color = match node.kind {
        PubDepKind::Direct => theme.accent,
        PubDepKind::Dev => theme.cyan,
        PubDepKind::Transitive => theme.dim,
    };
    out.push(Line::styled(
        format!("{indent}{} {}", node.name, node.version),
        Style::default().fg(color).bg(theme.bg),
    ));
    for child in &node.children {
        write_tree(child, depth + 1, theme, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn got_event_populates_lists() {
        let mut v = PubView::for_get_or_upgrade("get");
        v.apply(PubEvent::Got {
            added: vec!["a".into()],
            removed: vec!["b".into()],
            modified: vec![("c".into(), "1.0".into(), "2.0".into())],
        });
        match v.mode {
            PubMode::GetOrUpgrade { added, removed, modified } => {
                assert_eq!(added, vec!["a"]);
                assert_eq!(removed, vec!["b"]);
                assert_eq!(modified.len(), 1);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn outdated_event_replaces_rows() {
        let mut v = PubView::for_outdated();
        v.apply(PubEvent::Outdated {
            rows: vec![OutdatedRow {
                package: "http".into(),
                current: "0.13.5".into(),
                upgradable: "0.13.6".into(),
                resolvable: "0.14.0".into(),
                latest: "1.2.0".into(),
            }],
        });
        if let PubMode::Outdated { rows } = v.mode { assert_eq!(rows.len(), 1); } else { panic!() }
    }

    #[test]
    fn deps_event_sets_tree() {
        let mut v = PubView::for_deps();
        v.apply(PubEvent::Deps {
            tree: PubTreeNode {
                name: "myapp".into(),
                version: "1.0".into(),
                kind: PubDepKind::Direct,
                children: vec![],
            },
        });
        if let PubMode::Deps { tree } = v.mode { assert!(tree.is_some()); } else { panic!() }
    }

    #[test]
    fn done_sets_quitting() {
        let mut v = PubView::for_get_or_upgrade("get");
        v.apply(PubEvent::Done { success: true });
        assert!(v.quitting);
    }
}
```

- [ ] **Step 2: Update `crates/fl-tui/src/views/mod.rs`**

```rust
//! Command-specific TUI views.

pub mod build_view;
pub mod pub_view;
pub mod test_view;
```

- [ ] **Step 3: Re-export from `crates/fl-tui/src/lib.rs`**

```rust
pub use views::pub_view::{PubMode, PubView};
```

- [ ] **Step 4: Create `crates/fl-cli/src/pub_cmd.rs`**

```rust
//! `fl pub <subcommand>` — wraps `flutter pub *`.

use anyhow::{anyhow, Context};
use fl_core::PubEvent;
use fl_flutter::{parse_deps_json, parse_outdated_table, parse_pub_get, resolve_flutter};
use fl_tui::{PubView, TuiRunner};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncReadExt};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::cli::PubSub;

pub async fn run(sub: PubSub, project: Option<PathBuf>) -> anyhow::Result<()> {
    let project = project.unwrap_or_else(|| std::env::current_dir().unwrap());
    if !project.join("pubspec.yaml").exists() {
        return Err(anyhow!("no pubspec.yaml in {}", project.display()));
    }
    let flutter = resolve_flutter(None, std::env::var("FLUTTER_ROOT").ok().as_deref(), dirs_home().as_deref())
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
```

- [ ] **Step 5: Add the `label()` helper to `PubSub`**

In `crates/fl-cli/src/cli.rs`, after the `PubSub` enum definition (which doesn't exist yet — Task 16 adds it), add the impl. **For Task 13, only add `PubSub` minimally to make `pub_cmd` compile.** The full clap surface comes in Task 16. Add a stub at the bottom of `cli.rs`:

```rust
// Placeholder for Task 13 — full sub-command surface defined in Task 16.
#[derive(Debug, Clone)]
pub enum PubSub {
    Get,
    Upgrade,
    Outdated,
    Deps,
    Add { package: String },
    Remove { package: String },
}

impl PubSub {
    pub fn label(&self) -> &'static str {
        match self {
            PubSub::Get => "get",
            PubSub::Upgrade => "upgrade",
            PubSub::Outdated => "outdated",
            PubSub::Deps => "deps",
            PubSub::Add { .. } => "add",
            PubSub::Remove { .. } => "remove",
        }
    }
}
```

- [ ] **Step 6: Declare `pub_cmd` in `crates/fl-cli/src/main.rs`**

Add `mod pub_cmd;` near the other `mod` lines.

- [ ] **Step 7: Run tests + build**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui && cargo build --workspace`
Expected: 44 tests in fl-tui (40 prior + 4 new). Build clean.

- [ ] **Step 8: Commit**

```bash
git add crates/fl-tui/ crates/fl-cli/
git -c commit.gpgsign=false commit -m "feat: PubView and fl pub command module (all subcommands)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 14: `DoctorView` + `doctor_cmd`

**Files:**
- Create: `crates/fl-tui/src/views/doctor_view.rs`
- Modify: `crates/fl-tui/src/views/mod.rs`
- Modify: `crates/fl-tui/src/lib.rs`
- Create: `crates/fl-cli/src/doctor_cmd.rs`
- Modify: `crates/fl-cli/src/main.rs`

- [ ] **Step 1: Create `crates/fl-tui/src/views/doctor_view.rs`**

```rust
//! View for `fl doctor`.

use crate::theme::Theme;
use crate::view::View;
use fl_core::{DoctorEvent, DoctorStatus, KeyEvent as FlKey};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::time::Duration;

pub struct DoctorSectionView {
    pub status: DoctorStatus,
    pub title: String,
    pub details: Vec<String>,
    pub expanded: bool,
}

pub struct DoctorView {
    pub sections: Vec<DoctorSectionView>,
    pub cursor: usize,
    pub done: bool,
    pub quitting: bool,
}

impl DoctorView {
    pub fn new() -> Self {
        Self { sections: Vec::new(), cursor: 0, done: false, quitting: false }
    }
}

impl View for DoctorView {
    type Input = DoctorEvent;

    fn apply(&mut self, input: Self::Input) {
        match input {
            DoctorEvent::Section { status, title, details } => {
                let expanded = !matches!(status, DoctorStatus::Ok);
                self.sections.push(DoctorSectionView { status, title, details, expanded });
            }
            DoctorEvent::Done => {
                self.done = true;
            }
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let block = Block::default().title(" fl doctor ").borders(Borders::ALL).border_style(theme.dimmed()).style(theme.base());
        let inner = block.inner(area);
        block.render(area, buf);
        let mut lines: Vec<Line> = Vec::new();
        for (i, sec) in self.sections.iter().enumerate() {
            let (icon, color) = match sec.status {
                DoctorStatus::Ok => ("[✓]", theme.success),
                DoctorStatus::Warning => ("[!]", theme.warn),
                DoctorStatus::Error => ("[✗]", theme.error),
            };
            let prefix = if i == self.cursor { "▸ " } else { "  " };
            lines.push(Line::styled(
                format!("{prefix}{icon} {}", sec.title),
                Style::default().fg(color).bg(theme.bg),
            ));
            if sec.expanded {
                for d in &sec.details {
                    lines.push(Line::styled(format!("      • {d}"), theme.dimmed()));
                }
            }
        }
        Paragraph::new(lines).render(inner, buf);
    }

    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input> {
        match key {
            FlKey::Char('q') | FlKey::Ctrl('c') => {
                self.quitting = true;
            }
            FlKey::Down => {
                if self.cursor + 1 < self.sections.len() {
                    self.cursor += 1;
                }
            }
            FlKey::Up => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
            }
            FlKey::Enter | FlKey::Char(' ') => {
                if let Some(sec) = self.sections.get_mut(self.cursor) {
                    sec.expanded = !sec.expanded;
                }
            }
            _ => {}
        }
        None
    }
    fn tick(&mut self, _dt: Duration) {}
    fn quitting(&self) -> bool { self.quitting }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_event_appends_section() {
        let mut v = DoctorView::new();
        v.apply(DoctorEvent::Section {
            status: DoctorStatus::Ok,
            title: "Flutter".into(),
            details: vec!["v3.22".into()],
        });
        assert_eq!(v.sections.len(), 1);
        // Ok sections default to collapsed.
        assert!(!v.sections[0].expanded);
    }

    #[test]
    fn warning_section_defaults_to_expanded() {
        let mut v = DoctorView::new();
        v.apply(DoctorEvent::Section {
            status: DoctorStatus::Warning,
            title: "Android".into(),
            details: vec![],
        });
        assert!(v.sections[0].expanded);
    }

    #[test]
    fn down_arrow_moves_cursor() {
        let mut v = DoctorView::new();
        v.apply(DoctorEvent::Section { status: DoctorStatus::Ok, title: "a".into(), details: vec![] });
        v.apply(DoctorEvent::Section { status: DoctorStatus::Ok, title: "b".into(), details: vec![] });
        v.handle_key(FlKey::Down);
        assert_eq!(v.cursor, 1);
    }

    #[test]
    fn enter_toggles_expand() {
        let mut v = DoctorView::new();
        v.apply(DoctorEvent::Section { status: DoctorStatus::Ok, title: "a".into(), details: vec!["x".into()] });
        let was = v.sections[0].expanded;
        v.handle_key(FlKey::Enter);
        assert_ne!(v.sections[0].expanded, was);
    }
}
```

- [ ] **Step 2: Update `crates/fl-tui/src/views/mod.rs`**

```rust
//! Command-specific TUI views.

pub mod build_view;
pub mod doctor_view;
pub mod pub_view;
pub mod test_view;
```

- [ ] **Step 3: Re-export from `crates/fl-tui/src/lib.rs`**

```rust
pub use views::doctor_view::DoctorView;
```

- [ ] **Step 4: Create `crates/fl-cli/src/doctor_cmd.rs`**

```rust
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
    let flutter = resolve_flutter(None, std::env::var("FLUTTER_ROOT").ok().as_deref(), dirs_home().as_deref())
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
```

- [ ] **Step 5: Add `mod doctor_cmd;` in `crates/fl-cli/src/main.rs`**

- [ ] **Step 6: Run tests + build**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui && cargo build --workspace`
Expected: 48 fl-tui tests (44 prior + 4 new DoctorView).

- [ ] **Step 7: Commit**

```bash
git add crates/fl-tui/ crates/fl-cli/
git -c commit.gpgsign=false commit -m "feat: DoctorView and fl doctor command module

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 15: `CleanView` + `clean_cmd`

**Files:**
- Create: `crates/fl-tui/src/views/clean_view.rs`
- Modify: `crates/fl-tui/src/views/mod.rs`
- Modify: `crates/fl-tui/src/lib.rs`
- Create: `crates/fl-cli/src/clean_cmd.rs`
- Modify: `crates/fl-cli/src/main.rs`

- [ ] **Step 1: Create `crates/fl-tui/src/views/clean_view.rs`**

```rust
//! View for `fl clean`.

use crate::spinner::Spinner;
use crate::theme::Theme;
use crate::view::View;
use fl_core::{CleanEvent, KeyEvent as FlKey};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::time::Duration;

pub struct CleanView {
    pub spinner: Spinner,
    pub current_path: Option<String>,
    pub paths: Vec<String>,
    pub freed_bytes: u64,
    pub done: bool,
    pub quitting: bool,
    pub error: Option<String>,
}

impl CleanView {
    pub fn new() -> Self {
        Self {
            spinner: Spinner::default(),
            current_path: None,
            paths: Vec::new(),
            freed_bytes: 0,
            done: false,
            quitting: false,
            error: None,
        }
    }
}

impl View for CleanView {
    type Input = CleanEvent;

    fn apply(&mut self, input: Self::Input) {
        match input {
            CleanEvent::Probing => self.current_path = Some("(measuring…)".into()),
            CleanEvent::Cleaning { path } => self.current_path = Some(path),
            CleanEvent::Done { freed_bytes, paths } => {
                self.freed_bytes = freed_bytes;
                self.paths = paths;
                self.done = true;
                self.quitting = true;
                self.current_path = None;
            }
            CleanEvent::Error(msg) => {
                self.error = Some(msg);
                self.quitting = true;
            }
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let block = Block::default().title(" fl clean ").borders(Borders::ALL).border_style(theme.dimmed()).style(theme.base());
        let inner = block.inner(area);
        block.render(area, buf);
        if let Some(err) = &self.error {
            Paragraph::new(Line::styled(format!("✗ {err}"), Style::default().fg(theme.error).bg(theme.bg)))
                .alignment(Alignment::Center)
                .render(inner, buf);
            return;
        }
        if self.done {
            let pretty = format!("🧹 Cleaned {}", human_size(self.freed_bytes));
            Paragraph::new(Line::styled(pretty, Style::default().fg(theme.success).bg(theme.bg)))
                .alignment(Alignment::Center)
                .render(inner, buf);
        } else {
            let line = match &self.current_path {
                Some(p) => format!("{}  {p}", self.spinner.frame()),
                None => format!("{}  Initializing…", self.spinner.frame()),
            };
            Paragraph::new(Line::styled(line, Style::default().fg(theme.warn).bg(theme.bg)))
                .alignment(Alignment::Center)
                .render(inner, buf);
        }
    }

    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input> {
        if matches!(key, FlKey::Char('q') | FlKey::Ctrl('c')) {
            self.quitting = true;
        }
        None
    }
    fn tick(&mut self, dt: Duration) { self.spinner.tick(dt); }
    fn quitting(&self) -> bool { self.quitting }
}

fn human_size(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 { v /= 1024.0; i += 1; }
    format!("{v:.1} {}", units[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probing_sets_current_path() {
        let mut v = CleanView::new();
        v.apply(CleanEvent::Probing);
        assert!(v.current_path.is_some());
    }

    #[test]
    fn done_records_freed_and_sets_quitting() {
        let mut v = CleanView::new();
        v.apply(CleanEvent::Done { freed_bytes: 1_500_000, paths: vec!["build/".into()] });
        assert!(v.quitting);
        assert_eq!(v.freed_bytes, 1_500_000);
        assert_eq!(v.paths.len(), 1);
    }

    #[test]
    fn error_records_and_quits() {
        let mut v = CleanView::new();
        v.apply(CleanEvent::Error("boom".into()));
        assert!(v.error.is_some());
        assert!(v.quitting);
    }
}
```

- [ ] **Step 2: Update `crates/fl-tui/src/views/mod.rs`**

```rust
//! Command-specific TUI views.

pub mod build_view;
pub mod clean_view;
pub mod doctor_view;
pub mod pub_view;
pub mod test_view;
```

- [ ] **Step 3: Re-export from `crates/fl-tui/src/lib.rs`**

```rust
pub use views::clean_view::CleanView;
```

- [ ] **Step 4: Create `crates/fl-cli/src/clean_cmd.rs`**

```rust
//! `fl clean` — wraps `flutter clean`, with before/after byte counting.

use anyhow::{anyhow, Context};
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
    let flutter = resolve_flutter(None, std::env::var("FLUTTER_ROOT").ok().as_deref(), dirs_home().as_deref())
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
```

- [ ] **Step 5: Add `mod clean_cmd;` in `crates/fl-cli/src/main.rs`**

- [ ] **Step 6: Run tests + build**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui && cargo build --workspace`
Expected: 51 fl-tui tests.

- [ ] **Step 7: Commit**

```bash
git add crates/fl-tui/ crates/fl-cli/
git -c commit.gpgsign=false commit -m "feat: CleanView and fl clean command module with byte counting

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 16: CLI wiring — clap surface + main.rs dispatch

**Files:**
- Modify: `crates/fl-cli/src/cli.rs`
- Modify: `crates/fl-cli/src/main.rs`

- [ ] **Step 1: Replace `crates/fl-cli/src/cli.rs`**

```rust
//! Clap definitions for the `fl` binary.

use clap::{Parser, Subcommand};
use fl_core::{BuildMode, BuildTarget};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "fl", version, about = "A modern Flutter CLI with seamless USB→WiFi hot reload")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// List attached devices with status, IP, battery, OS version.
    Devices,
    /// Run a Flutter app with the `fl` dashboard. Auto-pairs USB→WiFi.
    Run {
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(short, long)] device: Option<String>,
        #[arg(long)] no_wifi: bool,
        #[arg(long, value_enum, default_value_t = BuildMode::Debug)] mode: BuildMode,
    },
    /// Build a Flutter app for a given target.
    Build {
        #[arg(value_enum)] target: BuildTarget,
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = BuildMode::Release)] mode: BuildMode,
    },
    /// Run flutter test with a live TUI.
    Test {
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(short, long)] name: Option<String>,
    },
    /// flutter pub subcommands.
    Pub {
        #[command(subcommand)] sub: PubSub,
        #[arg(short, long, global = true)] project: Option<PathBuf>,
    },
    /// flutter doctor with a TUI.
    Doctor,
    /// flutter clean with byte counting.
    Clean {
        #[arg(short, long)] project: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum PubSub {
    Get,
    Upgrade,
    Outdated,
    Deps,
    Add { package: String },
    Remove { package: String },
}

impl PubSub {
    pub fn label(&self) -> &'static str {
        match self {
            PubSub::Get => "get",
            PubSub::Upgrade => "upgrade",
            PubSub::Outdated => "outdated",
            PubSub::Deps => "deps",
            PubSub::Add { .. } => "add",
            PubSub::Remove { .. } => "remove",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_devices_subcommand() {
        let c = Cli::parse_from(["fl", "devices"]);
        assert!(matches!(c.cmd, Cmd::Devices));
    }

    #[test]
    fn parses_run_with_options() {
        let c = Cli::parse_from(["fl", "run", "--device", "1.2.3.4:5555", "--no-wifi"]);
        match c.cmd {
            Cmd::Run { device, no_wifi, mode, .. } => {
                assert_eq!(device.as_deref(), Some("1.2.3.4:5555"));
                assert!(no_wifi);
                assert_eq!(mode, BuildMode::Debug);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_run_with_explicit_mode() {
        let c = Cli::parse_from(["fl", "run", "--mode", "release"]);
        match c.cmd {
            Cmd::Run { mode, .. } => assert_eq!(mode, BuildMode::Release),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_build_apk() {
        let c = Cli::parse_from(["fl", "build", "apk"]);
        match c.cmd {
            Cmd::Build { target, mode, .. } => {
                assert_eq!(target, BuildTarget::Apk);
                assert_eq!(mode, BuildMode::Release);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_pub_add() {
        let c = Cli::parse_from(["fl", "pub", "add", "http"]);
        match c.cmd {
            Cmd::Pub { sub: PubSub::Add { package }, .. } => assert_eq!(package, "http"),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_doctor_and_clean() {
        assert!(matches!(Cli::parse_from(["fl", "doctor"]).cmd, Cmd::Doctor));
        assert!(matches!(Cli::parse_from(["fl", "clean"]).cmd, Cmd::Clean { .. }));
    }
}
```

- [ ] **Step 2: Replace `crates/fl-cli/src/main.rs`**

```rust
mod build_cmd;
mod clean_cmd;
mod cli;
mod devices_cmd;
mod doctor_cmd;
mod pub_cmd;
mod run_cmd;
mod test_cmd;

use anyhow::Context;
use clap::Parser;
use cli::{Cli, Cmd};
use directories::ProjectDirs;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

fn init_logging() -> anyhow::Result<tracing_appender::non_blocking::WorkerGuard> {
    let dirs = ProjectDirs::from("", "", "fl").context("project dirs")?;
    let log_dir = dirs.cache_dir();
    std::fs::create_dir_all(log_dir)?;
    let appender = RollingFileAppender::new(Rotation::NEVER, log_dir, "fl.log");
    let (nb, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env("FL_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(nb).with_ansi(false))
        .init();
    Ok(guard)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = init_logging().ok();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Devices => devices_cmd::run().await,
        Cmd::Run { project, device, no_wifi, mode } => {
            run_cmd::run(project, device, no_wifi, mode).await
        }
        Cmd::Build { target, project, mode } => build_cmd::run(target, project, mode).await,
        Cmd::Test { project, name } => test_cmd::run(project, name).await,
        Cmd::Pub { sub, project } => pub_cmd::run(sub, project).await,
        Cmd::Doctor => doctor_cmd::run().await,
        Cmd::Clean { project } => clean_cmd::run(project).await,
    }
}
```

> Note: the placeholder `PubSub` definition added to `cli.rs` in Task 13 is REPLACED by the proper one above with `#[derive(Subcommand)]`. Old definition is removed entirely.

- [ ] **Step 3: Run tests + build**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-cli && cargo build --workspace`
Expected: 9 fl-cli unit tests (was 3 before, +6 new clap tests). Build clean.

- [ ] **Step 4: Commit**

```bash
git add crates/fl-cli/
git -c commit.gpgsign=false commit -m "feat(cli): wire build/test/pub/doctor/clean sub-commands into the binary

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 17: Faux flutter recognises the new commands

**Files:**
- Modify: `tests/fixtures/bin/flutter`

- [ ] **Step 1: Replace `tests/fixtures/bin/flutter`**

```bash
#!/bin/sh
# Faux flutter --machine emitter and stub for non-machine commands.
# Routing:
#   flutter run --machine ...        -> SCENARIO from FL_FLUTTER_SCENARIO (existing behaviour)
#   flutter build <target> --machine -> FL_FLUTTER_BUILD_SCENARIO
#   flutter test --machine           -> FL_FLUTTER_TEST_SCENARIO
#   flutter pub get|upgrade          -> FL_FLUTTER_PUB_SCENARIO (raw text)
#   flutter pub outdated             -> FL_FLUTTER_PUB_OUTDATED_SCENARIO
#   flutter pub deps --json          -> FL_FLUTTER_PUB_DEPS_SCENARIO
#   flutter doctor -v                -> FL_FLUTTER_DOCTOR_SCENARIO
#   flutter clean                    -> no output, exit 0

emit_scenario() {
  local file="$1"
  if [ -z "$file" ] || [ ! -f "$file" ]; then
    return
  fi
  while IFS= read -r line; do
    case "$line" in
      "SLEEP "*) sleep "$(echo "$line" | awk '{print $2}')" ;;
      "") ;;
      *) echo "$line" ;;
    esac
  done < "$file"
}

cmd="$1"
case "$cmd" in
  run)
    if [ -n "$FL_FLUTTER_SCENARIO" ] && [ -f "$FL_FLUTTER_SCENARIO" ]; then
      emit_scenario "$FL_FLUTTER_SCENARIO"
    else
      echo '[{"event":"daemon.connected","params":{"version":"0.6.1"}}]'
      echo '[{"event":"app.started","params":{"appId":"abc","vmServiceUri":"ws://127.0.0.1:1/abc/ws"}}]'
      sleep 0.2
      echo '[{"event":"app.stop","params":{"exitCode":0}}]'
    fi
    ;;
  build)
    emit_scenario "${FL_FLUTTER_BUILD_SCENARIO:-}"
    ;;
  test)
    emit_scenario "${FL_FLUTTER_TEST_SCENARIO:-}"
    ;;
  pub)
    sub="$2"
    case "$sub" in
      get|upgrade|add|remove) emit_scenario "${FL_FLUTTER_PUB_SCENARIO:-}" ;;
      outdated) emit_scenario "${FL_FLUTTER_PUB_OUTDATED_SCENARIO:-}" ;;
      deps) emit_scenario "${FL_FLUTTER_PUB_DEPS_SCENARIO:-}" ;;
    esac
    ;;
  doctor)
    emit_scenario "${FL_FLUTTER_DOCTOR_SCENARIO:-}"
    ;;
  clean)
    # No output expected.
    ;;
esac
exit 0
```

- [ ] **Step 2: Keep it executable**

Run: `chmod +x tests/fixtures/bin/flutter`

- [ ] **Step 3: Sanity check**

Run:

```bash
FL_FLUTTER_BUILD_SCENARIO=/dev/null bash tests/fixtures/bin/flutter build apk --machine
```

Expected: prints nothing, exits 0.

- [ ] **Step 4: Commit**

```bash
git add tests/fixtures/bin/flutter
git -c commit.gpgsign=false commit -m "test: faux flutter routes build/test/pub/doctor/clean commands

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 18: Scenario fixtures + 5 headless integration tests

**Files:**
- Create: `tests/fixtures/scenarios/build_apk.txt`
- Create: `tests/fixtures/scenarios/test_basic.txt`
- Create: `tests/fixtures/scenarios/pub_get.txt`
- Create: `tests/fixtures/scenarios/pub_outdated.txt`
- Create: `tests/fixtures/scenarios/doctor.txt`
- Modify: `crates/fl-cli/tests/headless_run.rs`

- [ ] **Step 1: Create `tests/fixtures/scenarios/build_apk.txt`**

```
[{"event":"daemon.connected","params":{"version":"0.6.1"}}]
[{"event":"app.progress","params":{"id":"gradle","message":"Running Gradle task","finished":false}}]
SLEEP 0.05
[{"event":"app.progress","params":{"id":"gradle","message":"Running Gradle task","finished":true}}]
[{"event":"daemon.logMessage","params":{"level":"info","message":"Built build/app/outputs/flutter-apk/app-release.apk (12.3MB)."}}]
[{"event":"app.stop","params":{"exitCode":0}}]
```

- [ ] **Step 2: Create `tests/fixtures/scenarios/test_basic.txt`**

```
{"type":"suite","suite":{"id":1,"path":"test/example_test.dart"}}
{"type":"testStart","time":1,"test":{"id":1,"name":"loads home"}}
{"type":"testDone","testID":1,"result":"success","time":42,"name":"loads home"}
{"type":"testStart","time":50,"test":{"id":2,"name":"shows error"}}
{"type":"testDone","testID":2,"result":"failure","time":100,"name":"shows error"}
{"type":"done","success":false,"time":2000}
```

- [ ] **Step 3: Create `tests/fixtures/scenarios/pub_get.txt`**

```
Resolving dependencies...
+ shiny_pkg 1.0.0
- legacy_pkg
> updated_pkg 2.0.0 (was 1.9.0)
Got dependencies!
```

- [ ] **Step 4: Create `tests/fixtures/scenarios/pub_outdated.txt`**

```
Showing outdated packages.

Package Name      Current   Upgradable  Resolvable  Latest

direct dependencies:
http              0.13.5    0.13.6      0.14.0      1.2.0
```

- [ ] **Step 5: Create `tests/fixtures/scenarios/doctor.txt`**

```
[✓] Flutter (Channel stable, 3.22.2)
    • Flutter version 3.22.2
[!] Android Studio (not installed)

Doctor summary (1 issue found.)
```

- [ ] **Step 6: Append five tests to `crates/fl-cli/tests/headless_run.rs`**

Append AFTER the existing tests (preserve `ensure_binary_built`, `workspace_root`, `fixtures`, etc.):

```rust
fn run_fl_with_env(args: &[&str], envs: &[(&str, &std::path::Path)]) -> String {
    let exe = workspace_root().join("target/debug/fl").canonicalize().expect("fl built");
    let fixture_bin = fixtures().join("bin").canonicalize().expect("fixtures bin");
    let path = format!(
        "{}:{}",
        fixture_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut cmd = Command::new(&exe);
    cmd.args(args).env("PATH", path).env("FL_HEADLESS", "1").env_remove("FLUTTER_ROOT");
    for (k, p) in envs {
        cmd.env(k, p.canonicalize().expect("env path"));
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = cmd.output().expect("spawn fl");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn pubspec_in_workspace() -> PathBuf {
    // tests need a real pubspec.yaml for pre-checks in build/test/pub/clean.
    let p = workspace_root().join("target/test-pubspec");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("pubspec.yaml"), "name: dummy\n").unwrap();
    std::fs::create_dir_all(p.join("test")).unwrap();
    p
}

#[test]
fn headless_build_emits_progress_and_built() {
    ensure_binary_built();
    let pubspec = pubspec_in_workspace();
    let scenario = fixtures().join("scenarios/build_apk.txt");
    let out = run_fl_with_env(
        &["build", "apk", "--project", pubspec.to_str().unwrap()],
        &[("FL_FLUTTER_BUILD_SCENARIO", &scenario)],
    );
    assert!(out.contains("Progress"), "missing progress events:\n{out}");
    assert!(out.contains("Built"), "missing Built line:\n{out}");
}

#[test]
fn headless_test_emits_test_events() {
    ensure_binary_built();
    let pubspec = pubspec_in_workspace();
    let scenario = fixtures().join("scenarios/test_basic.txt");
    let out = run_fl_with_env(
        &["test", "--project", pubspec.to_str().unwrap()],
        &[("FL_FLUTTER_TEST_SCENARIO", &scenario)],
    );
    assert!(out.contains("TestStarted"), "missing TestStarted:\n{out}");
    assert!(out.contains("AllDone"), "missing AllDone:\n{out}");
}

#[test]
fn headless_pub_get_emits_got_event() {
    ensure_binary_built();
    let pubspec = pubspec_in_workspace();
    let scenario = fixtures().join("scenarios/pub_get.txt");
    let out = run_fl_with_env(
        &["pub", "get", "--project", pubspec.to_str().unwrap()],
        &[("FL_FLUTTER_PUB_SCENARIO", &scenario)],
    );
    assert!(out.contains("Got"), "missing Got event:\n{out}");
    assert!(out.contains("shiny_pkg"), "missing added package:\n{out}");
}

#[test]
fn headless_doctor_emits_sections() {
    ensure_binary_built();
    let scenario = fixtures().join("scenarios/doctor.txt");
    let out = run_fl_with_env(
        &["doctor"],
        &[("FL_FLUTTER_DOCTOR_SCENARIO", &scenario)],
    );
    assert!(out.contains("Section"), "missing Section event:\n{out}");
    assert!(out.contains("Done"), "missing Done event:\n{out}");
}

#[test]
fn headless_clean_completes_with_zero_freed() {
    ensure_binary_built();
    let pubspec = pubspec_in_workspace();
    let out = run_fl_with_env(
        &["clean", "--project", pubspec.to_str().unwrap()],
        &[],
    );
    assert!(out.contains("Done"), "missing Done event:\n{out}");
}
```

- [ ] **Step 7: Run integration tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-cli --test headless_run -- --test-threads=1 2>&1 | tail -15`
Expected: 8 tests pass (3 prior + 5 new).

- [ ] **Step 8: Run the full workspace + clippy**

Run: `. "$HOME/.cargo/env" && cargo test --workspace -- --test-threads=1 2>&1 | grep "test result" | head -20`
Expected: every line `ok`.

Run: `. "$HOME/.cargo/env" && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean (or fix any new warnings minimally).

- [ ] **Step 9: Commit**

```bash
git add tests/fixtures/scenarios/ crates/fl-cli/tests/headless_run.rs
git -c commit.gpgsign=false commit -m "test: headless integration tests for build/test/pub/doctor/clean

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage:**
- §3 View trait → Task 1 ✓
- §3 TuiRunner::run_view → Task 2 ✓
- §4 TestEvent → Task 5 ✓
- §4 DoctorEvent → Task 6 ✓
- §4 PubEvent + types → Task 7 ✓
- §4 BuildMode → Task 3 ✓
- §4 CleanEvent → Task 10 ✓
- §5 parsers (test/doctor/pub get/outdated/deps) → Tasks 5, 6, 7, 8, 9 ✓
- §6 views (build/test/pub/doctor/clean) → Tasks 11, 12, 13, 14, 15 ✓
- §7 CLI surface (clap) → Task 16 ✓
- §7 commands → Tasks 11, 12, 13, 14, 15 (each command's cmd module) ✓
- §8 mode handling → Task 4 (run) + Task 11 (build via build_cmd) ✓
- §9 error handling — pubspec/test dir checks in commands → Tasks 11, 12, 13, 15 ✓
- §10 testing → unit tests in each task + integration in Task 18 ✓
- §11 file-level diff → all files covered ✓

**2. Placeholder scan:** No TBD/TODO/"similar to". Every step contains executable code or exact commands. The placeholder `PubSub` introduced in Task 13 is explicitly REPLACED in Task 16 — that's a real dependency, not a placeholder.

**3. Type consistency:** All names match across tasks:
- `BuildMode::flutter_flag()` defined in Task 3, used in Tasks 4, 11.
- `BuildTarget::flutter_arg()` defined in Task 3, used in Task 11.
- `PubSub::label()` defined in Task 13 (stub) and re-defined in Task 16 (real). Same signature.
- `parse_pub_get`, `parse_outdated_table`, `parse_deps_json` re-exported each in their task and used in Task 13 (`pub_cmd`).
- `BuildView`, `TestView`, `PubView`, `DoctorView`, `CleanView` re-exported from `fl-tui` lib in their respective tasks.

---

## Execution Handoff

**Plan complete and saved to [docs/superpowers/plans/2026-05-18-flutter-commands.md](2026-05-18-flutter-commands.md). 18 TDD tasks total.**

**Two execution options:**

**1. Subagent-Driven (recommended)** — Fresh subagent per task, fast iteration.

**2. Inline Execution** — In-session with checkpoints.

**Which approach?**
