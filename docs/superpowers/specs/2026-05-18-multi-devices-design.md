# Multi-device support for `fl run`

**Status:** Approved design — Sub-project C
**Date:** 2026-05-18
**Depends on:** MVP 1, Sub-project A (failover hardening), Sub-project B (Flutter commands + modes), all shipped
**Out of scope:** iOS device specifics (sub-project D — handled by D's adapter), per-device hot reload targeting, multi-project setups, iOS↔Android mixed sessions (planned but blocked on D)

## 1. Goal

Let `fl run` drive multiple devices simultaneously from a single dashboard. A user with phone + tablet + emulator picks the devices interactively, sees all their logs in one merged stream, and broadcasts hot reload to every device at once.

The design choice locked in during brainstorming:
- **Hybrid model.** Each device gets its own `flutter run --machine` process and VM Service client; UI is single, broadcast-based.
- **Interactive picker** at startup when 2+ devices are present and no explicit `-d`/`--all`.
- **Broadcast keys**: `r`, `R`, `b`, `p`, `o`, `P` go to every device's VM Service in parallel.

## 2. Stack additions

None. We reuse `tokio::sync::mpsc`, the existing `ReconnectManager`, `FlutterDaemon`, `VmServiceClient`, and `View`/`TuiRunner` infrastructure.

## 3. CLI surface

`fl run` gains:
- `--device <id>` becomes **repeatable** (was singular). Each occurrence adds one target serial.
- `--all`: skip picker, run on every detected device.
- `--no-picker`: skip picker even when N ≥ 2; the runner falls back to the auto-pair-then-run path used for the first detected USB device (preserves a non-interactive default for scripts).

Picker is shown when **all three** of the following are true:
1. No `--device` was passed
2. `--all` was not passed
3. `--no-picker` was not passed
4. `adb devices -l` reports two or more devices

If `--device` is passed once or more, the runner uses exactly that set (no picker, no fallback).

The `Run` clap arm becomes:

```rust
Run {
    #[arg(short, long)] project: Option<PathBuf>,
    #[arg(short, long)] device: Vec<String>,
    #[arg(long)] all: bool,
    #[arg(long)] no_picker: bool,
    #[arg(long)] no_wifi: bool,
    #[arg(long, value_enum, default_value_t = BuildMode::Debug)] mode: BuildMode,
}
```

## 4. New picker view

`crates/fl-tui/src/views/device_picker.rs` — a `DevicePickerView` implementing `View<Input = DevicePickerInput>`:

```rust
pub enum DevicePickerInput {
    DeviceFound(Device),       // pushed by the runner as devices arrive
    Toggle(usize),             // from key handler
    SelectAll,
    Confirm,
    Cancel,
}

pub enum DevicePickerOutcome {
    Picked(Vec<String>),       // serials, in display order
    Cancelled,
}

pub struct DevicePickerView {
    pub devices: Vec<(Device, bool /* checked */)>,
    pub cursor: usize,
    pub outcome: Option<DevicePickerOutcome>,
}
```

Key bindings:
- `↑` / `↓`: move cursor
- `Space`: toggle current row
- `a`: select all
- `Enter`: emit `Picked(<selected serials>)`; quit returns to runner
- `q` / `Ctrl+C`: emit `Cancelled`; runner exits non-zero

Layout (from §2 mockup): bordered box titled `fl run ── Select devices`, one row per device with `[✓]`/`[ ]` checkbox, name, connection type, serial. Footer with shortcuts. Pre-selection: **none** — the user must consciously pick at least one. Empty-confirm beeps (`crossterm::style::Print("\x07")`-like effect) and stays on the picker.

The runner waits for `Picked` or `Cancelled` then proceeds.

## 5. Runtime architecture

The current single-device runtime becomes a multi-device runtime. The unit is `DeviceSession`:

```rust
// crates/fl-cli/src/run_cmd.rs (or new helper module)
pub struct DeviceSession {
    pub serial: String,
    pub short_name: String,       // ≤ 8-char tag for log prefixes
    pub display_name: String,     // model from `adb shell getprop`
    pub daemon: FlutterDaemon,
    pub vm_client: Option<VmServiceClient>,
    pub isolate_id: Option<String>,
    pub reconnect: Option<fl_adb::ManagerHandle>,
    pub state: DeviceSessionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceSessionState { Connecting, Ready, Reloading, Stopped, Failed }
```

The orchestrator (formerly `run_cmd::run`) becomes:

```
1. resolve_flutter, parse_devices_l, decide picker vs auto vs explicit.
2. For each selected serial in parallel:
   - pre_pair_wifi if it's a USB serial and --no-wifi is unset
   - resolve device name (adb shell getprop)
   - spawn FlutterDaemon with the project + mode-flag
   - spawn ReconnectManager (Sub-project A) for this device
   - subscribe to its FlutterEvent stream
3. Spawn one shared track-devices watcher (Sub-project A code, unchanged).
4. Build AppState with `active_sessions: Vec<DeviceSession>` and start TuiRunner.
5. Keys translate to broadcast: `r` → `join_all(sessions.iter().map(|s| s.vm_client?.hot_reload(s.isolate_id?)))`.
6. On `q`, send_quit to every daemon in parallel and wait with 3-second timeout each.
```

A new helper module `fl-cli/src/multi.rs` hosts `DeviceSession`, `spawn_session(...)`, and `broadcast_key(...)`. `run_cmd.rs` shrinks to picker decision + session-set assembly + TUI invocation.

## 6. AppState changes

`AppState` gains a `Vec<DeviceSession>` (visible to the TUI) and **loses** the `active_device` / `backup_device` fields. Existing callers that read those become callers of `active_sessions.iter()`:

```rust
pub struct AppState {
    // unchanged...
    pub active_sessions: Vec<DeviceSession>,   // replaces active_device + backup_device
    // ...
}
```

Migration:
- `apply_device(Discovered(d))` no longer flips active/backup; it just records the device on the matching session if a session for that serial exists (state transitions `Connecting → Ready`).
- `apply_device(Lost { serial })` marks the matching session `Stopped` (or, if the ReconnectManager is active, `Reloading`-equivalent reconnect-in-progress — but we keep `state` simple: only `Stopped` after the reconnect fails terminally).
- Logs handling unchanged in structure, but the producer prefixes each `FlutterEvent::Log.message` with `[short_name] ` before sending. The `apply_flutter` arm therefore needs no change.

### Short-name derivation

```rust
fn short_name(d: &Device) -> String {
    let base = d.model.as_deref().or(Some(&d.serial)).unwrap();
    base.chars().filter(|c| c.is_alphanumeric()).take(8).collect::<String>()
}
```

Example: "Pixel_8" → "Pixel8", "iPhone 15" → "iPhone1", "192.168.1.42:5555" → "1921681" (ugly but unique).

A deterministic color is picked per short_name from `[theme.accent, theme.cyan, theme.success, theme.warn]` via a simple djb2 hash modulo 4 — keeps prefix colors stable across runs.

## 7. Devices panel changes

The MVP `panels/devices.rs::render_devices` currently shows `active_device` + `backup_device` lines. It becomes:

```rust
pub fn render_devices(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    // header block stays
    let mut lines: Vec<Line> = Vec::new();
    for sess in &state.active_sessions {
        lines.extend(render_session_lines(sess, theme));
    }
    if state.active_sessions.is_empty() {
        lines.push(Line::styled("(aucun)", theme.dimmed()));
    }
    // ...
}
```

Each session renders two lines: a status row (`● Pixel 8   🔗 WiFi   ✓ ready`) and an address row (`  192.168.1.42:5555` or `  ABC123`). The reconnecting indicator (sub-project A) still applies — it surfaces under the session whose target matches the active reconnect target.

## 8. Performance panel adjustment

When `active_sessions.len() == 1`, the existing single-panel layout is unchanged. When `== 2`, the panel is split horizontally into two sub-panels (one per session). When `>= 3`, the panel collapses to a summary:

```
Avg FPS  ▁▂▃▅▆█  58.4   ·  3 devices online
Mem      ~140MB total
```

Per-session sparklines stay accessible via a future "drill-down" key (out of scope for this sub-project).

## 9. Broadcast key handling

In the orchestrator, after `TuiRunner::run` returns, OR in a dedicated key-handler task spawned during setup:

```rust
async fn broadcast_key(key: FlKey, sessions: &[DeviceSession], events: &mpsc::Sender<AppEvent>) {
    let calls = sessions.iter().filter_map(|s| {
        let client = s.vm_client.clone()?;
        let iso = s.isolate_id.clone()?;
        let short_name = s.short_name.clone();
        Some(async move {
            let res = match key {
                FlKey::Char('r') => client.hot_reload(&iso).await,
                FlKey::Char('R') => client.hot_restart(&iso).await,
                FlKey::Char('b') => client.toggle_brightness(&iso, true).await,
                FlKey::Char('p') => client.toggle_debug_paint(&iso, true).await,
                FlKey::Char('o') => client.toggle_platform(&iso, false).await,
                FlKey::Char('P') => client.toggle_performance_overlay(&iso, true).await,
                _ => return None,
            };
            Some((short_name, res))
        })
    });
    let results = futures_util::future::join_all(calls).await;
    for outcome in results.into_iter().flatten() {
        if let (short_name, Ok(_)) = outcome.clone() {
            events.send(AppEvent::Flutter(FlutterEvent::Log {
                level: LogLevel::Info,
                message: format!("[{short_name}] reload OK"),
            })).await.ok();
        } else if let (short_name, Err(e)) = outcome {
            events.send(AppEvent::Flutter(FlutterEvent::Log {
                level: LogLevel::Error,
                message: format!("[{short_name}] reload failed: {e}"),
            })).await.ok();
        }
    }
}
```

> Note: the snippet uses `Option<(String, Result)>` rather than the more idiomatic `Result<...>` so we can attach the short_name to the result; tightened in the implementation if simpler.

## 10. Errors

| Situation | Behaviour |
|---|---|
| 0 device detected | Waiting screen (existing MVP) |
| 1 device detected | Skip picker, behave like MVP single-device |
| Picker confirmed with 0 selected | Visual beep, stay on picker |
| Picker cancelled (`q`) | Exit with code 0, no banner |
| `--all` with 0 devices | Error before TUI: "no devices" |
| 1 of N sessions fails to spawn | Banner: `1/3 sessions failed`, others proceed |
| All N fail | Exit with code 1, error overlay |
| Hot reload fails on 1 of N | Per-device error log line, others still log success |
| Quit with stuck daemon | 3-second timeout per session, then `kill()` |
| Device disconnected mid-session | Existing reconnect manager kicks in for that session only |

## 11. Testing strategy

### Unit (fl-tui)

- `DevicePickerView`:
  - `toggle` flips the checked flag for the indexed device
  - `select_all` checks every device
  - `confirm` with selection emits `Picked(serials)` in display order
  - `confirm` with empty selection does **not** emit (stays put)
- `AppState`:
  - `apply_device(Discovered)` of a serial that matches a session transitions `Connecting → Ready`
  - `apply_device(Lost)` transitions matching session to `Stopped`
- Snapshot test of the multi-device devices panel (2 sessions, 1 ready, 1 connecting)

### Unit (fl-cli)

- clap: `--device a --device b` → `Run { device: ["a","b"], .. }`
- clap: `--all` parses
- clap: mutually-exclusive validation between `--device` and `--all` (clap arg group)

### Integration (headless)

`headless_multi_device`:
- Faux `adb devices -l` lists 2 USB devices
- `fl run --all --no-wifi --no-picker` (bypass interactive screen)
- Faux flutter runs the existing scenario for **each** device (both should emit `app.started` + `app.stop`)
- Assert the dump contains `AppStarted` at least 2 times and `Stopped` at least 2 times

The headless mode handles the picker by being treated like `--no-picker` automatically — when `FL_HEADLESS=1`, the picker is unconditionally skipped (fail-safe).

## 12. File-level diff summary

| File | Change |
|---|---|
| `crates/fl-core/src/events.rs` | + `DeviceSessionState` enum |
| `crates/fl-tui/src/app.rs` | Replace `active_device`/`backup_device` with `active_sessions: Vec<DeviceSession>`; update `apply_device` accordingly; short-name + colour helpers |
| `crates/fl-tui/src/panels/devices.rs` | Render N sessions instead of fixed two |
| `crates/fl-tui/src/panels/performance.rs` | Split / summary layout per session count |
| `crates/fl-tui/src/views/device_picker.rs` | **new** picker view |
| `crates/fl-tui/src/views/mod.rs` | + `pub mod device_picker;` |
| `crates/fl-tui/src/lib.rs` | + re-export `DevicePickerView`, `DeviceSession`, `DeviceSessionState` |
| `crates/fl-cli/src/cli.rs` | `device: Vec<String>`, `all: bool`, `no_picker: bool`; clap arg-group `device` xor `all`; new tests |
| `crates/fl-cli/src/main.rs` | Adapt destructuring |
| `crates/fl-cli/src/run_cmd.rs` | Replace single-device path with `multi::run` |
| `crates/fl-cli/src/multi.rs` | **new** — `DeviceSession`, `spawn_session`, `broadcast_key`, `run_multi` |
| `tests/fixtures/bin/adb` | Optional: support `FL_ADB_FIXTURE_DEVICES` file with 2 devices for multi-device scenarios (already supported) |
| `crates/fl-cli/tests/headless_run.rs` | + `headless_multi_device` test |

The `DeviceSession` struct lives in `fl-cli/src/multi.rs` (not `fl-core` or `fl-tui`) because it owns Tokio handles tied to the binary's runtime. `AppState`'s `active_sessions` field carries a lightweight projection (`Vec<DeviceSessionSummary>` containing serial, short_name, display_name, state, ip) — defined in `fl-tui::app` and populated from `multi::DeviceSession` via a periodic sync event.

Refinement: rather than coupling AppState to `multi::DeviceSession` (which would create a cyclic crate dep), the runner emits `AppEvent::Device(DeviceEvent::SessionState { serial, state })` and `Discovered` events; AppState rebuilds its `Vec<DeviceSessionSummary>` from those. This preserves the MVP one-way data flow.

## 13. Open assumptions

- All selected devices run **the same Flutter project**. Per-device project paths are out of scope.
- Hot reload is broadcast-only. No per-device targeting (`r1`, `r2`) in this iteration.
- The `--mode` flag (sub-project B) is the same for every selected device. Different modes per device is a non-goal.
- When sub-project A's `ReconnectManager` triggers a successful reconnect, the session transitions back to `Ready`. We do not retry sessions that hit terminal `Failed`.
