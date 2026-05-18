# iOS / Apple device support for `fl run`

**Status:** Approved design — Sub-project D
**Date:** 2026-05-18
**Depends on:** MVP 1, sub-projects A + B + C all shipped
**Out of scope:** iOS reconnect manager (no equivalent of `adb connect`), iOS signing/provisioning helpers, custom simulator bootstrap

## 1. Goal

Add iOS / watchOS / tvOS / visionOS (and iOS simulators) to `fl run` discovery and execution. The user with a Flutter project that targets iOS can pick their iPhone in the multi-device picker alongside an Android device and run on both at once with `r` broadcasting hot reload.

The architecture choices locked in during brainstorming:
- **`xcrun`-based discovery** — no external dependency.
- **Two sources:** `xcrun devicectl list devices --json-output -` (physical Apple devices) + `xcrun simctl list devices --json` (simulators).
- **Polling watcher**, 3-second cadence — no equivalent of `adb track-devices` for iOS.
- **No iOS-side pre-pair or reconnect** — iOS pairing is handled by Xcode once and stays paired.

## 2. Stack additions

None. `xcrun` is built into macOS; JSON parsing reuses `serde_json` already in scope.

## 3. New crate `fl-ios`

```
crates/fl-ios/
├── Cargo.toml
└── src/
    ├── lib.rs          # re-exports
    ├── xcrun.rs        # CommandRunner-backed shells for devicectl + simctl
    ├── parse.rs        # JSON parsers → Vec<Device>
    └── watcher.rs      # polling loop + diff emitter
```

The crate depends on `fl-core` (for `Device`, `DeviceEvent`, `ConnectionKind`) and `fl-adb` (for `CommandRunner` trait + `MockRunner` test double, *only*; no Android logic is invoked). Re-using `fl-adb::CommandRunner` avoids duplicating the trait; we could later extract it to `fl-core` but that's churn.

### Public API

```rust
// fl-ios/src/lib.rs
pub use parse::{parse_devicectl_json, parse_simctl_json};
pub use watcher::{list_apple_devices, watch_apple_devices, diff_devices};
pub use xcrun::Xcrun;
```

### `Xcrun` (`xcrun.rs`)

Thin wrapper around `CommandRunner` to keep the call sites readable:

```rust
pub struct Xcrun<R: CommandRunner> { runner: R }

impl<R: CommandRunner> Xcrun<R> {
    pub fn new(runner: R) -> Self { Self { runner } }

    pub async fn devicectl_list(&self) -> anyhow::Result<String> {
        let out = self.runner.run("xcrun", &["devicectl", "list", "devices", "--json-output", "-"]).await?;
        Ok(out.stdout)
    }

    pub async fn simctl_list(&self) -> anyhow::Result<String> {
        let out = self.runner.run("xcrun", &["simctl", "list", "devices", "--json"]).await?;
        Ok(out.stdout)
    }
}
```

If `xcrun` isn't on PATH, `runner.run` returns an `Err`; callers treat that as "no iOS support available" — log a warn, return `Vec::new()`.

### `parse.rs`

```rust
pub fn parse_devicectl_json(json: &str) -> Vec<Device> { ... }
pub fn parse_simctl_json(json: &str) -> Vec<Device> { ... }
```

Both return `Vec<Device>`. Logic:

- `parse_devicectl_json`:
  - Top-level JSON path: `result.devices[]`
  - For each: `identifier` → `serial`, `deviceProperties.name` → `name`, `deviceProperties.platform` lower-cased → `platform`, `connectionProperties.transportType == "wired"` → `Usb` else `Wifi`, `deviceProperties.osVersionNumber` → `android_version` (reused as generic "os version" string), `state` → `Online` if `connectionProperties.tunnelState == "connected"` else `Offline`.
- `parse_simctl_json`:
  - Top-level JSON path: `devices` is an object keyed by runtime; iterate values, filter `state == "Booted" && isAvailable == true`. `udid` → `serial`, `name` → `name`, `connection = Usb` (irrelevant for sims but a placeholder), `platform = "ios-simulator"`, `state = Online`.

Both functions tolerate malformed JSON (return `Vec::new()`).

### `watcher.rs`

```rust
pub async fn list_apple_devices<R: CommandRunner>(xcrun: &Xcrun<R>) -> Vec<Device> {
    let mut devs = Vec::new();
    if let Ok(j) = xcrun.devicectl_list().await { devs.extend(parse_devicectl_json(&j)); }
    if let Ok(j) = xcrun.simctl_list().await { devs.extend(parse_simctl_json(&j)); }
    devs
}

pub async fn watch_apple_devices<R>(xcrun: Xcrun<R>, tx: mpsc::Sender<DeviceEvent>)
where R: CommandRunner + Send + Sync + 'static
{
    let mut prev: HashMap<String, Device> = HashMap::new();
    let mut in_flight = false;
    loop {
        if !in_flight {
            in_flight = true;
            let cur = list_apple_devices(&xcrun).await;
            in_flight = false;
            let cur_map: HashMap<String, Device> = cur.iter().cloned().map(|d| (d.serial.clone(), d)).collect();
            for ev in diff_devices(&prev, &cur) { tx.send(ev).await.ok(); }
            prev = cur_map;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

pub fn diff_devices(prev: &HashMap<String, Device>, cur: &[Device]) -> Vec<DeviceEvent> {
    let cur_map: HashMap<&str, &Device> = cur.iter().map(|d| (d.serial.as_str(), d)).collect();
    let mut events = Vec::new();
    for new in cur {
        if !prev.contains_key(&new.serial) {
            events.push(DeviceEvent::Discovered(new.clone()));
        }
    }
    for old_serial in prev.keys() {
        if !cur_map.contains_key(old_serial.as_str()) {
            events.push(DeviceEvent::Lost { serial: old_serial.clone() });
        }
    }
    events
}
```

> Note: the spec text mentioned a `Mutex<bool> in_flight` for slow xcrun calls. In practice the polling loop runs sequentially — there's nothing to mutex. The `in_flight` flag in `watch_apple_devices` is a noop variable here for clarity; remove if Clippy complains.

## 4. Type changes

### `fl-core::Device`

Add an optional `platform` field at the end:

```rust
pub struct Device {
    pub serial: String,
    pub name: String,
    pub model: Option<String>,
    pub connection: ConnectionKind,
    pub state: DeviceState,
    pub ip: Option<String>,
    pub android_version: Option<String>,
    pub battery: Option<u8>,
    pub platform: Option<String>,   // NEW: "android" | "ios" | "ios-simulator" | "watchos" | "tvos"
}
```

All existing constructors must add `platform: None` to compile. The `fl-adb::parse::parse_devices_l` sets `platform: Some("android".into())`. The `fl-ios` parsers set their own.

### `fl-core::DeviceSessionSummary`

Add a mirrored `platform: Option<String>` so the panel can render it without looking up the source `Device`:

```rust
pub struct DeviceSessionSummary {
    pub serial: String,
    pub short_name: String,
    pub display_name: String,
    pub connection: ConnectionKind,
    pub ip: Option<String>,
    pub state: DeviceSessionState,
    pub platform: Option<String>,  // NEW
}
```

`AppState::apply_device(SessionState {...})` initializes `platform: None`; when a `Discovered` arrives, copy `d.platform.clone()` onto the summary.

## 5. Panel changes

In `fl-tui/src/panels/devices.rs`, the per-session row gets a platform tag immediately before the connection icon. Render:

```
● [iPhone15 ] iPhone 15  ios       🔗 WiFi  ready
    192.168.1.20
○ [Pixel8   ] Pixel 8    android   ⚡ USB    stopped
    ABC123
```

A single 9-character padded slot for the platform label keeps columns aligned. Missing platform falls back to a blank slot. Implementation: extend the existing row construction in `lines_for(session, theme)` with:

```rust
let plat = session.platform.as_deref().unwrap_or("");
// ... add a Span for `format!("{plat:<9} ", )` between display_name and icon.
```

No other panel changes.

## 6. Multi-device orchestrator update (`fl-cli::multi`)

Two extension points:

1. **Discovery.** In `run_multi`, after `parse_devices_l(adb_output)` runs, also call `fl_ios::list_apple_devices(&Xcrun::new(runner.clone()))` and concatenate the results into `all_devices`.

2. **Pre-pair branching.** In `spawn_session`, replace the existing logic:

```rust
let final_target = if let (Some(usb), false) = (usb_serial_to_pair.as_deref(), no_wifi) { ... }
```

with platform-aware logic. The simplest path: only set `usb_serial_to_pair = Some(...)` for devices whose `platform == "android"` or whose platform is `None` (back-compat). The call site that decides whether to pass `usb_serial_to_pair` lives in `run_multi`'s loop where it picks the matching device:

```rust
let usb_pair = all_devices
    .iter()
    .find(|d| d.serial == *serial
              && matches!(d.connection, ConnectionKind::Usb)
              && (d.platform.as_deref() == Some("android") || d.platform.is_none()))
    .map(|d| d.serial.clone());
```

iOS devices on USB get `usb_pair = None`, so `pre_pair_wifi` is skipped.

3. **Watcher.** After spawning `track_devices` (Android), also spawn `watch_apple_devices` with the same `event_tx`. Both push `DeviceEvent::Discovered/Lost` into the same channel; `AppState` handles them uniformly.

## 7. Picker view update

`DevicePickerView` already iterates `Device`s. With `platform` available, render an extra column:

```
[ ] iPhone 15           ios       WiFi · 00008140-XXXX
[✓] Pixel 8             android   USB · ABC123
[ ] iPhone 15 Pro Sim   ios-sim   USB · DEAD-BEEF
```

(`ios-sim` is `ios-simulator` truncated for display.)

Update the render line format inside `DevicePickerView::render` to include `d.platform.as_deref().unwrap_or("")`:

```rust
format!("{arrow}{bullet} {:<22} {:<9} {} · {}",
    d.name,
    d.platform.as_deref().map(|p| if p == "ios-simulator" { "ios-sim" } else { p }).unwrap_or(""),
    conn, d.serial)
```

## 8. Errors

| Situation | Behaviour |
|---|---|
| `xcrun` not installed (no Xcode CLT) | `Xcrun::devicectl_list` returns Err; `list_apple_devices` returns `Vec::new()`; no iOS devices show up. Log warn once. |
| `xcrun devicectl list` runs but JSON is empty / malformed | `parse_devicectl_json` returns `Vec::new()`. No panic. |
| iOS device in Developer-Mode-disabled state | `tunnelState != "connected"` → `state = Offline`. Picker still shows it, user can attempt; Flutter will report the real error. |
| Simulator in `Shutdown` state | Filtered out by `parse_simctl_json`. Reappears once Booted. |
| `xcrun` hangs (rare) | Polling loop is sequential and awaits; max latency = `xcrun` timeout from OS. In practice `xcrun devicectl` returns < 1s; if it ever blocks for > 3s the next poll just runs late. Not a fatal issue. |
| iOS device disconnects mid-session | `watch_apple_devices` emits `Lost`; `AppState` marks the session `Stopped`. No reconnect attempt. |

## 9. Testing strategy

### Unit tests (`fl-ios`)

`parse.rs`:
- `parse_devicectl_json_two_devices` — fixture JSON with one wired iPhone and one wireless iPad → two `Device`s, correct `ConnectionKind`, `platform = Some("ios")`.
- `parse_devicectl_json_developer_mode_disabled` — single device with `tunnelState != "connected"` → `state = Offline`.
- `parse_simctl_json_filters_shutdown` — fixture with 1 Booted + 1 Shutdown → 1 device returned, `platform = Some("ios-simulator")`.
- `parse_*_json_malformed_returns_empty` — invalid JSON inputs → `Vec::new()`.

`watcher.rs`:
- `diff_devices_emits_discovered_for_new_serial`
- `diff_devices_emits_lost_for_dropped_serial`

### Integration test

A faux `xcrun` shell script is added to `tests/fixtures/bin/` (the existing fixture bin dir). It routes:

- `xcrun devicectl list devices --json-output -` → cat `$FL_XCRUN_DEVICECTL_SCENARIO`
- `xcrun simctl list devices --json` → cat `$FL_XCRUN_SIMCTL_SCENARIO`

New scenario fixtures `tests/fixtures/scenarios/ios_one_device.json` and `tests/fixtures/scenarios/sim_one_booted.json`.

A new headless test `headless_ios_run_emits_app_started` (in `crates/fl-cli/tests/headless_run.rs`) drives `fl run --device 00008140-XXXX --no-picker --no-wifi` against a fixture that:
1. Faux `adb` reports no Android devices.
2. Faux `xcrun devicectl` reports one iPhone.
3. Faux `flutter` plays the nominal scenario.

The test asserts `AppStarted` and no `pre-pair failed` event (iOS shouldn't pre-pair).

## 10. File-level diff summary

| File | Change |
|---|---|
| `crates/fl-core/src/events.rs` | + `Device::platform`, + `DeviceSessionSummary::platform` |
| `crates/fl-adb/src/parse.rs` | parse_devices_l sets `platform = Some("android".into())` |
| `crates/fl-adb/src/watcher.rs` | parse_track_payload sets `platform = Some("android".into())` |
| `crates/fl-ios/Cargo.toml` | **new** |
| `crates/fl-ios/src/lib.rs` | **new** |
| `crates/fl-ios/src/xcrun.rs` | **new** |
| `crates/fl-ios/src/parse.rs` | **new** |
| `crates/fl-ios/src/watcher.rs` | **new** |
| `crates/fl-tui/src/app.rs` | propagate `platform` into summaries |
| `crates/fl-tui/src/panels/devices.rs` | show platform tag column |
| `crates/fl-tui/src/views/device_picker.rs` | show platform tag column |
| `crates/fl-cli/Cargo.toml` | + `fl-ios = { path = "../fl-ios" }` |
| `crates/fl-cli/src/multi.rs` | merge `fl_ios::list_apple_devices` into discovery; spawn `watch_apple_devices`; iOS-aware `usb_pair` decision |
| `crates/fl-cli/src/devices_cmd.rs` | also list iOS devices in `fl devices` (small bonus) |
| `Cargo.toml` (workspace) | + `crates/fl-ios` member |
| `tests/fixtures/bin/xcrun` | **new** faux script |
| `tests/fixtures/scenarios/ios_one_device.json` | **new** |
| `tests/fixtures/scenarios/sim_one_booted.json` | **new** |
| `crates/fl-cli/tests/headless_run.rs` | + `headless_ios_run_emits_app_started` |

## 11. Open assumptions

- `xcrun devicectl` JSON shape is the one shipped with Xcode 15.x. If Apple changes the schema in Xcode 16+, the parser fails-soft (returns `Vec::new()`); a follow-up updates the parser.
- We assume `xcrun` is on PATH. Users without Xcode get zero iOS devices, which is the correct behaviour.
- iOS device names from `devicectl` are user-set (Settings → General → About → Name). Acceptable. Truncation to 22 chars happens in the picker layout.
- The `android_version` field is reused as a generic "os version" string for iOS too. A rename to `os_version` is deferred to a future cleanup pass to avoid cascading edits in this sub-project.

## 12. Out of scope

- `fl ios pair` / wireless debug helpers — Xcode is the source of truth.
- iOS-side ReconnectManager — no equivalent of `adb connect`.
- Simulator lifecycle (boot/shutdown/erase) — user manages via Xcode or `xcrun simctl boot` outside of `fl`.
- `flutter run` provisioning helpers — Flutter already handles signing via Xcode.
