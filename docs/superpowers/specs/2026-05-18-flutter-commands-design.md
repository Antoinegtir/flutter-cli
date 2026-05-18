# Flutter commands and build modes — `fl build` / `test` / `pub` / `doctor` / `clean`

**Status:** Approved design — Sub-project B
**Date:** 2026-05-18
**Depends on:** MVP 1 + Sub-project A (failover hardening) already shipped
**Out of scope:** Multi-device (sub-project C), iOS device support (sub-project D)

## 1. Goal

Add five new sub-commands to the `fl` binary with command-specific TUIs, plus a `--mode` flag on `fl run` and `fl build`:

| Sub-command | What it wraps | View flavour |
|---|---|---|
| `fl build <target>` | `flutter build apk\|aab\|ios\|web --machine` | Phase list + binary size |
| `fl test` | `flutter test --machine` | Live counters + failure stacks |
| `fl pub <subcommand>` | `flutter pub get\|upgrade\|outdated\|add\|remove\|deps` | Tailored per subcommand |
| `fl doctor` | `flutter doctor -v` | Coloured sections with collapse |
| `fl clean` | `flutter clean` + before/after byte count | Animated summary |

Mode flag:
- `fl run --mode debug\|profile\|release` (default `debug`)
- `fl build <target> --mode debug\|profile\|release` (default `release`)

## 2. Stack additions

None. All five Flutter sub-commands already work without new dependencies. New parsing uses `serde_json` (already in scope) for `--machine` outputs and hand-written regex for the plain-text ones.

## 3. View trait — making `TuiRunner` generic

The MVP `TuiRunner::run` is hardcoded to `&mut AppState` + a `Receiver<AppEvent>`. To host five other views without copy-paste, we introduce a small trait:

```rust
// crates/fl-tui/src/view.rs
pub trait View: Send {
    /// Type of event this view consumes (e.g. AppEvent, BuildEvent, TestEvent).
    type Input: Send + 'static;

    fn apply(&mut self, input: Self::Input);
    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme);

    /// Translate a terminal key into a view-specific input, or `None` to ignore.
    /// Returning a value DOES NOT mean the view will see it via `apply` — the runner
    /// echoes it back through `apply` after `handle_key` returns, so the same code
    /// path handles external and internal inputs.
    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input>;

    fn tick(&mut self, dt: Duration);
    fn quitting(&self) -> bool;
}
```

`AppState` (the existing run view) implements `View<Input = AppEvent>`. Each new command has its own struct (e.g. `BuildView`) implementing `View<Input = BuildInput>`.

`TuiRunner` gains a generic `run_view<V: View>` method. Its body is the same render-loop / event-drain as the existing `run`, parameterised over `V`. The existing `run` stays as a thin wrapper that calls `run_view::<AppState>` so that `run_cmd.rs` doesn't change.

## 4. New event types

### `fl-core::TestEvent`

```rust
pub enum TestEvent {
    SuiteStart { path: String },
    TestStarted { id: u64, name: String },
    TestDone { id: u64, name: String, result: TestResult, duration_ms: u64 },
    Error { id: Option<u64>, message: String, stack: Option<String> },
    AllDone { success: bool, passed: u32, failed: u32, skipped: u32 },
}

pub enum TestResult { Success, Failure, Error, Skipped }
```

### `fl-core::DoctorEvent`

```rust
pub enum DoctorEvent {
    Section { status: DoctorStatus, title: String, details: Vec<String> },
    Done,
}
pub enum DoctorStatus { Ok, Warning, Error }
```

### `fl-core::PubEvent`

```rust
pub enum PubEvent {
    Resolving,
    Got { added: Vec<String>, removed: Vec<String>, modified: Vec<(String, String, String)> },
    Outdated { rows: Vec<OutdatedRow> },
    Deps { tree: PubTreeNode },
    Log { level: LogLevel, message: String },
    Done { success: bool },
}
pub struct OutdatedRow {
    pub package: String,
    pub current: String,
    pub upgradable: String,
    pub resolvable: String,
    pub latest: String,
}
pub struct PubTreeNode {
    pub name: String,
    pub version: String,
    pub kind: PubDepKind,  // Direct, Dev, Transitive
    pub children: Vec<PubTreeNode>,
}
```

### `fl-core::CleanEvent`

```rust
pub enum CleanEvent {
    Probing,
    Cleaning { path: String },
    Done { freed_bytes: u64, paths: Vec<String> },
    Error(String),
}
```

`BuildEvent` is **not** introduced — `flutter build --machine` emits the same `app.progress`/`app.stop`/`daemon.logMessage` events as `flutter run`, so the existing `FlutterEvent` is reused. The build view consumes `FlutterEvent` directly.

### `fl-core::BuildMode`

```rust
pub enum BuildMode { Debug, Profile, Release }
impl BuildMode {
    pub fn flutter_flag(self) -> &'static str {
        match self { Self::Debug => "--debug", Self::Profile => "--profile", Self::Release => "--release" }
    }
}
```

## 5. New parsers in `fl-flutter`

### `fl-flutter/src/test_parse.rs`

Parses lines from `flutter test --machine`. Each line is a JSON object (not wrapped in `[…]` like the daemon protocol). Five recognised `type` fields: `start`, `suite`, `testStart`, `testDone`, `error`, `done`.

`parse_test_line(raw: &str) -> Option<TestEvent>` mirrors `parse_daemon_line`.

### `fl-flutter/src/doctor_parse.rs`

`flutter doctor -v` produces sections of the form:

```
[✓] Flutter (Channel stable, 3.22.2, on macOS 14.5 ...)
    • Flutter version 3.22.2 ...
    • Engine revision ...
[!] Android Studio (not installed)
[✗] Xcode (not installed)
```

`parse_doctor_section(lines: impl Iterator<Item=&str>) -> Vec<DoctorSection>`. Match the bracketed marker character to map `✓` → `Ok`, `!` → `Warning`, `✗` → `Error`. Indented lines (starting with whitespace and `•`) attach to the most recent section. Plain-text `Doctor summary: ...` is the terminator.

### `fl-flutter/src/pub_parse.rs`

Three helpers:

- `parse_pub_get(stdout: &str) -> PubEvent::Got` — Look for the line `Resolving dependencies...` then `+ package_name 1.0.0` (added), `- package_name` (removed), and `> package_name 1.0.0 (was 0.9.0)` (modified) patterns.
- `parse_outdated_table(stdout: &str) -> Vec<OutdatedRow>` — `flutter pub outdated` prints a fixed-width table with columns `Package`, `Current`, `Upgradable`, `Resolvable`, `Latest`. Split on 2+ spaces, skip header and separator rows.
- `parse_deps_json(json: &str) -> Result<PubTreeNode>` — Use `serde_json` to parse the structured output of `flutter pub deps --json` and recurse into the `dependencies` array to build a tree. Direct/dev/transitive comes from the top-level `directDependencies`, `devDependencies`, and `dependencies` arrays.

## 6. New TUI views (in `fl-tui/src/views/`)

| File | View | `Input` |
|---|---|---|
| `views/build_view.rs` | `BuildView` | `FlutterEvent` |
| `views/test_view.rs` | `TestView` | `TestEvent` |
| `views/pub_view.rs` | `PubView` | `PubEvent` (variants drive the layout switch) |
| `views/doctor_view.rs` | `DoctorView` | `DoctorEvent` |
| `views/clean_view.rs` | `CleanView` | `CleanEvent` |

Each view is its own struct with a state and a `render()` that draws a panel for that command. All use the existing `Theme::TOKYO_NIGHT` palette and the same spinner/progress primitives.

### `BuildView`

State: target name, mode, ordered `Vec<BuildStep>` (id, message, status: pending/running/done/failed), final binary path, final size. Render:

```
╭─ fl build ─── apk · release ────────────╮
│  ✓ Initializing gradle      0.8s        │
│  ✓ Resolving dependencies   2.3s        │
│  ⠹ Running Gradle task      ...         │
│    Preparing tree                       │
│    ─────────────────────────────────    │
│    Built build/.../app-release.apk      │
│    Size: 12.3 MB · 18s total            │
╰─────────────────────────────────────────╯
```

### `TestView`

State: counters (`passed/failed/skipped`), running test ids/names, completed test names (last 20), failures with stack traces. Render: top counter (large), middle live list, bottom expand of failures with `j`/`k` to navigate.

### `PubView`

Variant-driven layout. For `get`/`upgrade`: spinner + bulleted list of added/removed/modified. For `outdated`: a table with cells coloured (current=dim, upgradable=warn yellow, resolvable=info cyan, latest=success green). For `deps`: indented tree with `direct` nodes bold and `transitive` dimmed.

### `DoctorView`

Vertical list of sections; each is a header line with status icon + collapsible block of details. Keys: `↑/↓` to navigate, `Enter`/`Space` to toggle details.

### `CleanView`

A spinner during `flutter clean`, then a centered summary `🧹 Cleaned 1.23 GB`. List of paths underneath.

### Refactor of existing `render::render`

The existing top-level `render` function becomes the `View::render` impl on `AppState`. The `panels::*` modules remain unchanged. No behavioural change for `fl run`.

## 7. CLI surface

### `fl-cli/src/cli.rs`

```rust
pub enum Cmd {
    Devices,
    Run {
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(short, long)] device: Option<String>,
        #[arg(long)] no_wifi: bool,
        #[arg(long, default_value = "debug")] mode: BuildMode,
    },
    Build {
        #[arg(value_enum)] target: BuildTarget,
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(long, default_value = "release")] mode: BuildMode,
    },
    Test {
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(short, long)] name: Option<String>,
    },
    Pub {
        #[command(subcommand)] sub: PubSub,
    },
    Doctor,
    Clean {
        #[arg(short, long)] project: Option<PathBuf>,
    },
}

pub enum BuildTarget { Apk, Aab, Ios, Web }

pub enum PubSub {
    Get,
    Upgrade,
    Outdated,
    Deps,
    Add { package: String },
    Remove { package: String },
}
```

`BuildMode` and `BuildTarget` derive `clap::ValueEnum` and live in `fl-core` (so other crates can match on them).

### `fl-cli/src/<cmd>_cmd.rs`

Each new module mirrors the shape of `run_cmd.rs`:

1. Resolve `flutter` path (existing `resolve_flutter`).
2. Spawn the Flutter sub-process with the right args (and `--machine` when supported).
3. Parse output line-by-line, push parsed events into a `tokio::sync::mpsc::Sender<V::Input>`.
4. Initialize a new `View` struct, hand it to `TuiRunner::run_view`.
5. On view `quitting()`, drain the child, restore the terminal, exit.

## 8. Mode handling

`fl run` — the existing `run_cmd::run(project, device, no_wifi)` becomes `run(project, device, no_wifi, mode)`. The mode flag is appended to the `flutter run` command line in `fl-flutter::FlutterDaemon::spawn` via a new `args` parameter (the current call uses `["run", "--machine", "-d", device_id]`; it becomes that plus the mode flag when not default).

For `fl build`, the spawn becomes `flutter build <target> --machine <mode-flag>`. Default release means no flag is added when the user does not pass `--mode`.

## 9. Error handling

| Situation | Behaviour |
|---|---|
| `fl build apk` outside a Flutter project | Detect missing `./pubspec.yaml` before spawn; clean error overlay. |
| `fl test` with no `test/` directory | Same upfront check; error overlay with hint `flutter test` would print. |
| `flutter pub add some_invalid_pkg` returns non-zero | View shows the stderr in a red panel, exit code propagated. |
| `flutter doctor` fails to spawn (`flutter` not found) | Same `resolve_flutter` path as MVP, error overlay. |
| `fl clean` and `build/` doesn't exist | Skip the probe, show "Nothing to clean (0 bytes freed)". |
| `--mode release` but signing isn't configured (Android) | Flutter daemon emits a `daemon.logMessage` error; surface in the build view's log panel as red, build view marks the failing step `✗`. |

All sub-commands use `?` to propagate fatal setup errors to `main`, which renders a final error message to stderr and exits with non-zero.

## 10. Testing strategy

### Unit tests

- `fl-flutter/src/test_parse.rs`: parse 5 fixture lines (start, suite, testStart success, testStart failure, done success).
- `fl-flutter/src/doctor_parse.rs`: parse 3 fixture sections (Flutter ✓ with details, Android !, Xcode ✗ no details).
- `fl-flutter/src/pub_parse.rs`: 3 fixtures (pub get, outdated, deps JSON).
- `fl-tui/src/views/*`: snapshot tests of mid-state and final-state renders, one per view.
- `fl-cli/src/cli.rs`: clap parsing for each new sub-command and `--mode` enum.

### Integration tests

Extend `tests/fixtures/bin/flutter` to recognise extra args (`build`, `test`, `pub`, `doctor`, `clean`) and emit canned fixture output from `tests/fixtures/scenarios/<cmd>.txt`. Add headless integration tests `headless_build`, `headless_test`, `headless_pub_get`, `headless_doctor`, `headless_clean` that assert key events appear in the dump.

### Manual smoke test

Each command is run against a real Flutter sample project. Tracked in `docs/SMOKE.md` (created in a later sub-project; for now just verified by hand).

## 11. File-level diff summary

| File | Change |
|---|---|
| `crates/fl-core/src/events.rs` | + `TestEvent`, `TestResult`, `DoctorEvent`, `DoctorStatus`, `PubEvent`, `OutdatedRow`, `PubTreeNode`, `PubDepKind`, `CleanEvent`, `BuildMode` |
| `crates/fl-flutter/src/test_parse.rs` | **new** |
| `crates/fl-flutter/src/doctor_parse.rs` | **new** |
| `crates/fl-flutter/src/pub_parse.rs` | **new** |
| `crates/fl-flutter/src/lib.rs` | + re-exports |
| `crates/fl-flutter/src/daemon.rs` | Spawn args parameterised; `FlutterDaemon::spawn` accepts extra args slice |
| `crates/fl-tui/src/view.rs` | **new** — `View` trait |
| `crates/fl-tui/src/runner.rs` | + `run_view<V: View>`; existing `run` becomes a thin wrapper |
| `crates/fl-tui/src/app.rs` | Implement `View<Input=AppEvent>` for `AppState` |
| `crates/fl-tui/src/views/` | **new** dir — `build_view.rs`, `test_view.rs`, `pub_view.rs`, `doctor_view.rs`, `clean_view.rs`, `mod.rs` |
| `crates/fl-cli/src/cli.rs` | + sub-commands, BuildTarget, PubSub, `--mode` flags |
| `crates/fl-cli/src/main.rs` | + new sub-command dispatch |
| `crates/fl-cli/src/run_cmd.rs` | + mode parameter, append mode flag to flutter args |
| `crates/fl-cli/src/build_cmd.rs` | **new** |
| `crates/fl-cli/src/test_cmd.rs` | **new** |
| `crates/fl-cli/src/pub_cmd.rs` | **new** |
| `crates/fl-cli/src/doctor_cmd.rs` | **new** |
| `crates/fl-cli/src/clean_cmd.rs` | **new** |
| `crates/fl-cli/tests/headless_run.rs` | + 5 new headless tests |
| `tests/fixtures/bin/flutter` | Accept `build`/`test`/`pub`/`doctor`/`clean` and dispatch to scenarios |
| `tests/fixtures/scenarios/build_*.txt`, `test_*.txt`, `pub_*.txt`, `doctor.txt`, `clean.txt` | **new** fixtures |

## 12. Open assumptions

- `flutter test --machine` JSON shape is captured from Flutter 3.22.x; if Flutter changes the protocol the test parser may need adjustment.
- `flutter pub outdated` ASCII table is the default output; we don't support a future `--json` flag (Flutter doesn't expose one yet).
- `flutter pub deps --json` is the documented machine-readable format; if absent in old Flutter versions, the deps view shows a graceful error.
- `flutter clean` doesn't print sizes; we compute them ourselves via `tokio::fs::metadata` recursive walk. The probe is best-effort — symlinks and permission errors are skipped silently.

## 13. Out of scope

- iOS device-side bits (sub-project D).
- Multi-device build (sub-project C — build runs per project, not per device, so the dimension doesn't apply).
- Custom flavours (`--flavor` arg) — could be a follow-up.
- `flutter analyze`, `flutter format`, `flutter create` — separate later.
