# iOS / Apple device support (Sub-project D) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make iOS, watchOS, tvOS, visionOS, and iOS simulators visible to `fl run` so the user can pick them in the multi-device picker and run Flutter on them alongside Android targets.

**Architecture:** A new `fl-ios` crate wraps `xcrun devicectl` (physical Apple devices) and `xcrun simctl` (simulators). A polling watcher (3-second cadence) feeds the same `DeviceEvent` channel that `fl-adb::track_devices` already feeds. `Device` and `DeviceSessionSummary` grow an optional `platform` string; the picker and devices panel render it as an extra column. `multi.rs` merges Android and Apple discovery, skipping `pre_pair_wifi` for iOS devices.

**Tech Stack:** Same as MVP 1 + sub-projects A/B/C. No new external deps. `xcrun` is built into macOS.

**Spec:** [docs/superpowers/specs/2026-05-18-ios-support-design.md](../specs/2026-05-18-ios-support-design.md)

---

## File Structure

```
Cargo.toml                                  # + member crates/fl-ios

crates/fl-core/src/events.rs                # + Device.platform, + DeviceSessionSummary.platform

crates/fl-adb/src/parse.rs                  # set platform="android" on parsed Android devices
crates/fl-adb/src/watcher.rs                # set platform="android" on track_payload devices

crates/fl-ios/
├── Cargo.toml                              # new
└── src/
    ├── lib.rs                              # new
    ├── xcrun.rs                            # new: Xcrun wrapper around CommandRunner
    ├── parse.rs                            # new: devicectl + simctl JSON parsers
    └── watcher.rs                          # new: list_apple_devices, diff_devices, watch_apple_devices

crates/fl-tui/src/app.rs                    # propagate platform into DeviceSessionSummary
crates/fl-tui/src/panels/devices.rs         # render platform tag column
crates/fl-tui/src/views/device_picker.rs    # render platform tag column

crates/fl-cli/Cargo.toml                    # + fl-ios dep
crates/fl-cli/src/multi.rs                  # merge xcrun devices, spawn watcher, platform-aware usb_pair

tests/fixtures/bin/xcrun                    # new faux script
tests/fixtures/scenarios/ios_one_device.json    # new
tests/fixtures/scenarios/sim_one_booted.json    # new
crates/fl-cli/tests/headless_run.rs         # + headless_ios_run_emits_app_started
```

---

## Task 1: Add `platform: Option<String>` to `Device` and `DeviceSessionSummary`

**Files:**
- Modify: `crates/fl-core/src/events.rs`

- [ ] **Step 1: Add field to `Device`** (in `crates/fl-core/src/events.rs`)

Find the `Device` struct and add the `platform` field at the end (preserve existing fields and order):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub serial: String,
    pub name: String,
    pub model: Option<String>,
    pub connection: ConnectionKind,
    pub state: DeviceState,
    pub ip: Option<String>,
    pub android_version: Option<String>,
    pub battery: Option<u8>,
    pub platform: Option<String>,
}
```

- [ ] **Step 2: Add field to `DeviceSessionSummary`**

Find the `DeviceSessionSummary` struct and add `platform: Option<String>`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceSessionSummary {
    pub serial: String,
    pub short_name: String,
    pub display_name: String,
    pub connection: ConnectionKind,
    pub ip: Option<String>,
    pub state: DeviceSessionState,
    pub platform: Option<String>,
}
```

- [ ] **Step 3: Fix the existing `device_equality_is_value_based` test in `mod tests`**

Find the test that constructs a `Device` literal and append `platform: None` to its initializer:

```rust
    #[test]
    fn device_equality_is_value_based() {
        let a = Device {
            serial: "S1".into(),
            name: "Pixel 8".into(),
            model: Some("Pixel 8".into()),
            connection: ConnectionKind::Usb,
            state: DeviceState::Online,
            ip: None,
            android_version: Some("14".into()),
            battery: Some(90),
            platform: None,
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
```

- [ ] **Step 4: Fix `device_session_summary_equality` test**

```rust
    #[test]
    fn device_session_summary_equality() {
        let s = DeviceSessionSummary {
            serial: "S".into(),
            short_name: "short".into(),
            display_name: "Pixel 8".into(),
            connection: ConnectionKind::Wifi,
            ip: Some("1.2.3.4".into()),
            state: DeviceSessionState::Connecting,
            platform: None,
        };
        let t = s.clone();
        assert_eq!(s, t);
    }
```

- [ ] **Step 5: Add a test asserting platform field roundtrips**

```rust
    #[test]
    fn device_platform_roundtrips_through_json() {
        let d = Device {
            serial: "X".into(),
            name: "x".into(),
            model: None,
            connection: ConnectionKind::Wifi,
            state: DeviceState::Online,
            ip: None,
            android_version: None,
            battery: None,
            platform: Some("ios".into()),
        };
        let j = serde_json::to_string(&d).unwrap();
        let back: Device = serde_json::from_str(&j).unwrap();
        assert_eq!(back.platform.as_deref(), Some("ios"));
    }
```

- [ ] **Step 6: Run tests** — they will FAIL TO COMPILE because every other `Device` and `DeviceSessionSummary` literal in the workspace is missing the new field.

Run: `. "$HOME/.cargo/env" && cargo build --workspace 2>&1 | grep "error\[E" | head -10`

Fix each error by appending the missing field initializers. The compiler error messages point at every site. Common sites and the value to use:

| File | Add to literal |
|---|---|
| `crates/fl-adb/src/parse.rs` (parse_devices_l) | `platform: Some("android".into()),` |
| `crates/fl-adb/src/watcher.rs` (parse_track_payload) | `platform: Some("android".into()),` |
| `crates/fl-adb/src/parse.rs` (tests using `Device { ... }`) | `platform: None,` |
| `crates/fl-adb/src/watcher.rs` (tests) | `platform: None,` |
| `crates/fl-cli/src/devices_cmd.rs` (test fixture) | `platform: None,` |
| `crates/fl-tui/src/app.rs` (apply_device's SessionState arm) | `platform: None,` |
| `crates/fl-tui/src/app.rs` (tests) | `platform: None,` |
| `crates/fl-tui/src/views/device_picker.rs` (test helpers) | `platform: None,` |

For `fl-adb::parse_devices_l` and `fl-adb::watcher::parse_track_payload`, set `Some("android".into())`. For all test/fixture constructions, set `None`.

Specifically in `fl-tui::app::apply_device` (the `SessionState` arm constructs a `DeviceSessionSummary`):

```rust
                    self.active_sessions.push(fl_core::DeviceSessionSummary {
                        serial: serial.clone(),
                        short_name: short_name_for_serial(&serial),
                        display_name: serial.clone(),
                        connection: if serial.contains(':') && serial.contains('.') {
                            fl_core::ConnectionKind::Wifi
                        } else {
                            fl_core::ConnectionKind::Usb
                        },
                        ip: None,
                        state,
                        platform: None,
                    });
```

And the `Discovered` arm in the same function should also propagate platform from the incoming device:

```rust
            DeviceEvent::Discovered(d) => {
                if let Some(sess) = self.active_sessions.iter_mut().find(|s| s.serial == d.serial) {
                    sess.state = fl_core::DeviceSessionState::Ready;
                    sess.ip = d.ip.clone();
                    sess.connection = d.connection;
                    sess.display_name = d.name.clone();
                    sess.platform = d.platform.clone();
                }
            }
```

- [ ] **Step 7: Re-run tests**

Run: `. "$HOME/.cargo/env" && cargo test --workspace -- --test-threads=1 2>&1 | grep "test result"`
Expected: all `ok`. Snapshot tests may need re-acceptance:

Run if needed: `. "$HOME/.cargo/env" && INSTA_UPDATE=always cargo test -p fl-tui dashboard_snapshot 2>&1 | tail -5`

- [ ] **Step 8: Commit**

```bash
git add crates/
git -c commit.gpgsign=false commit -m "feat(core): Device.platform and DeviceSessionSummary.platform optional fields

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Create `fl-ios` crate skeleton + `Xcrun` wrapper

**Files:**
- Create: `crates/fl-ios/Cargo.toml`
- Create: `crates/fl-ios/src/lib.rs`
- Create: `crates/fl-ios/src/xcrun.rs`
- Modify: `Cargo.toml` (workspace) to add the new member

- [ ] **Step 1: Add `crates/fl-ios` to workspace `members` in the root `Cargo.toml`**

The current members list is:
```
members = [
    "crates/fl-core",
    "crates/fl-adb",
    "crates/fl-flutter",
    "crates/fl-vmservice",
    "crates/fl-tui",
    "crates/fl-cli",
]
```

Add `"crates/fl-ios"` after `"crates/fl-adb"`:
```
members = [
    "crates/fl-core",
    "crates/fl-adb",
    "crates/fl-ios",
    "crates/fl-flutter",
    "crates/fl-vmservice",
    "crates/fl-tui",
    "crates/fl-cli",
]
```

- [ ] **Step 2: Create `crates/fl-ios/Cargo.toml`**

```toml
[package]
name = "fl-ios"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[dependencies]
fl-core = { path = "../fl-core" }
fl-adb = { path = "../fl-adb" }
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
async-trait.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["test-util"] }
```

- [ ] **Step 3: Create `crates/fl-ios/src/xcrun.rs`**

```rust
//! Thin wrapper around `xcrun` invocations via `CommandRunner`.

use fl_adb::CommandRunner;

pub struct Xcrun<R: CommandRunner> {
    runner: R,
}

impl<R: CommandRunner> Xcrun<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    pub async fn devicectl_list(&self) -> anyhow::Result<String> {
        let out = self
            .runner
            .run("xcrun", &["devicectl", "list", "devices", "--json-output", "-"])
            .await?;
        Ok(out.stdout)
    }

    pub async fn simctl_list(&self) -> anyhow::Result<String> {
        let out = self.runner.run("xcrun", &["simctl", "list", "devices", "--json"]).await?;
        Ok(out.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fl_adb::{CommandOutput, MockRunner};

    #[tokio::test]
    async fn devicectl_list_invokes_correct_command() {
        let m = MockRunner::new();
        m.expect("xcrun devicectl list devices --json-output -", CommandOutput::ok("{\"x\":1}"));
        let x = Xcrun::new(m);
        let out = x.devicectl_list().await.unwrap();
        assert_eq!(out, "{\"x\":1}");
    }

    #[tokio::test]
    async fn simctl_list_invokes_correct_command() {
        let m = MockRunner::new();
        m.expect("xcrun simctl list devices --json", CommandOutput::ok("{\"y\":2}"));
        let x = Xcrun::new(m);
        let out = x.simctl_list().await.unwrap();
        assert_eq!(out, "{\"y\":2}");
    }
}
```

- [ ] **Step 4: Create `crates/fl-ios/src/lib.rs`**

```rust
//! Apple device discovery via `xcrun devicectl` and `xcrun simctl`.

pub mod xcrun;

pub use xcrun::Xcrun;
```

- [ ] **Step 5: Build + test**

Run: `. "$HOME/.cargo/env" && cargo build -p fl-ios && cargo test -p fl-ios`
Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/fl-ios/
git -c commit.gpgsign=false commit -m "feat(ios): new fl-ios crate skeleton with Xcrun wrapper

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `parse_devicectl_json`

**Files:**
- Create: `crates/fl-ios/src/parse.rs` (start with devicectl only)
- Modify: `crates/fl-ios/src/lib.rs`

- [ ] **Step 1: Create `crates/fl-ios/src/parse.rs`**

```rust
//! Parsers for `xcrun devicectl` and `xcrun simctl` JSON outputs.

use fl_core::{ConnectionKind, Device, DeviceState};
use serde_json::Value;

/// Parse `xcrun devicectl list devices --json-output -` into `Device`s.
/// Top-level path: `result.devices[]`.
pub fn parse_devicectl_json(raw: &str) -> Vec<Device> {
    let v: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let devices = match v.get("result").and_then(|r| r.get("devices")).and_then(Value::as_array) {
        Some(d) => d,
        None => return Vec::new(),
    };
    devices.iter().filter_map(parse_devicectl_entry).collect()
}

fn parse_devicectl_entry(entry: &Value) -> Option<Device> {
    let identifier = entry.get("identifier").and_then(Value::as_str)?.to_string();
    let props = entry.get("deviceProperties")?;
    let name = props.get("name").and_then(Value::as_str)?.to_string();
    let platform_raw = props.get("platform").and_then(Value::as_str).unwrap_or("iOS").to_string();
    let platform = platform_raw.to_ascii_lowercase();
    let os_version = props.get("osVersionNumber").and_then(Value::as_str).map(str::to_string);

    let conn = entry.get("connectionProperties");
    let connection = match conn.and_then(|c| c.get("transportType")).and_then(Value::as_str) {
        Some("wired") => ConnectionKind::Usb,
        _ => ConnectionKind::Wifi,
    };
    let tunnel_connected = conn
        .and_then(|c| c.get("tunnelState"))
        .and_then(Value::as_str)
        .map(|s| s == "connected")
        .unwrap_or(true);
    let state = if tunnel_connected { DeviceState::Online } else { DeviceState::Offline };

    Some(Device {
        serial: identifier.clone(),
        name,
        model: None,
        connection,
        state,
        ip: None,
        android_version: os_version,
        battery: None,
        platform: Some(platform),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_DEVICES: &str = r#"{
        "result": {
            "devices": [
                {
                    "identifier": "00008140-001234567890",
                    "deviceProperties": {
                        "name": "iPhone 15",
                        "osVersionNumber": "17.4.1",
                        "platform": "iOS"
                    },
                    "connectionProperties": {
                        "transportType": "wired",
                        "tunnelState": "connected"
                    }
                },
                {
                    "identifier": "00008110-ABCDEF",
                    "deviceProperties": {
                        "name": "iPad Pro",
                        "osVersionNumber": "17.4",
                        "platform": "iPadOS"
                    },
                    "connectionProperties": {
                        "transportType": "wireless",
                        "tunnelState": "connected"
                    }
                }
            ]
        }
    }"#;

    #[test]
    fn parse_devicectl_json_two_devices() {
        let v = parse_devicectl_json(TWO_DEVICES);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].serial, "00008140-001234567890");
        assert_eq!(v[0].name, "iPhone 15");
        assert_eq!(v[0].connection, ConnectionKind::Usb);
        assert_eq!(v[0].platform.as_deref(), Some("ios"));
        assert_eq!(v[0].android_version.as_deref(), Some("17.4.1"));
        assert_eq!(v[1].connection, ConnectionKind::Wifi);
        assert_eq!(v[1].platform.as_deref(), Some("ipados"));
    }

    #[test]
    fn parse_devicectl_json_developer_mode_disabled_marks_offline() {
        let raw = r#"{"result":{"devices":[{
            "identifier":"X","deviceProperties":{"name":"iPhone","platform":"iOS"},
            "connectionProperties":{"transportType":"wired","tunnelState":"disconnected"}
        }]}}"#;
        let v = parse_devicectl_json(raw);
        assert_eq!(v[0].state, DeviceState::Offline);
    }

    #[test]
    fn parse_devicectl_json_malformed_returns_empty() {
        assert!(parse_devicectl_json("").is_empty());
        assert!(parse_devicectl_json("not json").is_empty());
        assert!(parse_devicectl_json(r#"{"unrelated": true}"#).is_empty());
    }
}
```

- [ ] **Step 2: Update `crates/fl-ios/src/lib.rs`**

```rust
//! Apple device discovery via `xcrun devicectl` and `xcrun simctl`.

pub mod parse;
pub mod xcrun;

pub use parse::parse_devicectl_json;
pub use xcrun::Xcrun;
```

- [ ] **Step 3: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-ios`
Expected: 5 tests pass (2 prior + 3 new).

- [ ] **Step 4: Commit**

```bash
git add crates/fl-ios/
git -c commit.gpgsign=false commit -m "feat(ios): parse_devicectl_json for xcrun devicectl output

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `parse_simctl_json`

**Files:**
- Modify: `crates/fl-ios/src/parse.rs`
- Modify: `crates/fl-ios/src/lib.rs`

- [ ] **Step 1: Append `parse_simctl_json` to `crates/fl-ios/src/parse.rs`** (above the test module)

```rust
/// Parse `xcrun simctl list devices --json` into `Device`s.
/// Filters to `state == "Booted" && isAvailable`.
/// Top-level path: `devices` is an object keyed by runtime; each value is an array.
pub fn parse_simctl_json(raw: &str) -> Vec<Device> {
    let v: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let runtimes = match v.get("devices").and_then(Value::as_object) {
        Some(o) => o,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for (_runtime, list) in runtimes {
        let Some(arr) = list.as_array() else { continue };
        for entry in arr {
            let state = entry.get("state").and_then(Value::as_str).unwrap_or("");
            let available = entry.get("isAvailable").and_then(Value::as_bool).unwrap_or(false);
            if state != "Booted" || !available {
                continue;
            }
            let Some(udid) = entry.get("udid").and_then(Value::as_str) else { continue };
            let name = entry.get("name").and_then(Value::as_str).unwrap_or(udid).to_string();
            out.push(Device {
                serial: udid.to_string(),
                name,
                model: None,
                connection: ConnectionKind::Usb,
                state: DeviceState::Online,
                ip: None,
                android_version: None,
                battery: None,
                platform: Some("ios-simulator".into()),
            });
        }
    }
    out
}
```

- [ ] **Step 2: Append tests to `mod tests`**

```rust
    const SIMCTL_TWO: &str = r#"{
        "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-17-4": [
                {
                    "udid": "BOOTED-1111",
                    "name": "iPhone 15 Pro",
                    "state": "Booted",
                    "isAvailable": true
                },
                {
                    "udid": "SHUTDOWN-2222",
                    "name": "iPhone 14",
                    "state": "Shutdown",
                    "isAvailable": true
                }
            ]
        }
    }"#;

    #[test]
    fn parse_simctl_json_filters_shutdown() {
        let v = parse_simctl_json(SIMCTL_TWO);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].serial, "BOOTED-1111");
        assert_eq!(v[0].name, "iPhone 15 Pro");
        assert_eq!(v[0].platform.as_deref(), Some("ios-simulator"));
    }

    #[test]
    fn parse_simctl_json_unavailable_is_filtered() {
        let raw = r#"{"devices":{"r":[{"udid":"x","name":"x","state":"Booted","isAvailable":false}]}}"#;
        assert!(parse_simctl_json(raw).is_empty());
    }

    #[test]
    fn parse_simctl_json_malformed_returns_empty() {
        assert!(parse_simctl_json("").is_empty());
        assert!(parse_simctl_json("nope").is_empty());
    }
```

- [ ] **Step 3: Update re-exports in `crates/fl-ios/src/lib.rs`**

```rust
pub use parse::{parse_devicectl_json, parse_simctl_json};
```

- [ ] **Step 4: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-ios`
Expected: 8 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/fl-ios/
git -c commit.gpgsign=false commit -m "feat(ios): parse_simctl_json for booted iOS simulators

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Watcher (`list_apple_devices`, `diff_devices`, `watch_apple_devices`)

**Files:**
- Create: `crates/fl-ios/src/watcher.rs`
- Modify: `crates/fl-ios/src/lib.rs`

- [ ] **Step 1: Create `crates/fl-ios/src/watcher.rs`**

```rust
//! Polling watcher for Apple devices via `xcrun`.

use crate::parse::{parse_devicectl_json, parse_simctl_json};
use crate::xcrun::Xcrun;
use fl_adb::CommandRunner;
use fl_core::{Device, DeviceEvent};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

/// Single-shot snapshot of Apple devices (devicectl + simctl combined).
pub async fn list_apple_devices<R: CommandRunner>(xcrun: &Xcrun<R>) -> Vec<Device> {
    let mut devs = Vec::new();
    if let Ok(j) = xcrun.devicectl_list().await {
        devs.extend(parse_devicectl_json(&j));
    }
    if let Ok(j) = xcrun.simctl_list().await {
        devs.extend(parse_simctl_json(&j));
    }
    devs
}

/// Compute the diff between previous and current Apple device sets.
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

/// Long-running polling loop. Polls every 3 seconds and emits Discovered/Lost diffs.
pub async fn watch_apple_devices<R>(xcrun: Xcrun<R>, tx: Sender<DeviceEvent>)
where
    R: CommandRunner + Send + Sync + 'static,
{
    let mut prev: HashMap<String, Device> = HashMap::new();
    loop {
        let cur = list_apple_devices(&xcrun).await;
        let cur_map: HashMap<String, Device> = cur.iter().cloned().map(|d| (d.serial.clone(), d)).collect();
        for ev in diff_devices(&prev, &cur) {
            tx.send(ev).await.ok();
        }
        prev = cur_map;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fl_adb::{CommandOutput, MockRunner};
    use fl_core::{ConnectionKind, DeviceState};

    fn dev(serial: &str) -> Device {
        Device {
            serial: serial.into(),
            name: serial.into(),
            model: None,
            connection: ConnectionKind::Usb,
            state: DeviceState::Online,
            ip: None,
            android_version: None,
            battery: None,
            platform: Some("ios".into()),
        }
    }

    #[test]
    fn diff_emits_discovered_for_new_serial() {
        let prev = HashMap::new();
        let cur = vec![dev("A")];
        let evs = diff_devices(&prev, &cur);
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], DeviceEvent::Discovered(_)));
    }

    #[test]
    fn diff_emits_lost_for_dropped_serial() {
        let mut prev = HashMap::new();
        prev.insert("A".into(), dev("A"));
        let cur: Vec<Device> = Vec::new();
        let evs = diff_devices(&prev, &cur);
        assert_eq!(evs.len(), 1);
        assert!(matches!(&evs[0], DeviceEvent::Lost { serial } if serial == "A"));
    }

    #[tokio::test]
    async fn list_apple_devices_combines_devicectl_and_simctl() {
        let m = MockRunner::new();
        m.expect(
            "xcrun devicectl list devices --json-output -",
            CommandOutput::ok(r#"{"result":{"devices":[{
                "identifier":"P","deviceProperties":{"name":"iPhone","platform":"iOS"},
                "connectionProperties":{"transportType":"wired","tunnelState":"connected"}
            }]}}"#),
        );
        m.expect(
            "xcrun simctl list devices --json",
            CommandOutput::ok(r#"{"devices":{"r":[
                {"udid":"S","name":"Sim","state":"Booted","isAvailable":true}
            ]}}"#),
        );
        let x = Xcrun::new(m);
        let all = list_apple_devices(&x).await;
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|d| d.serial == "P"));
        assert!(all.iter().any(|d| d.serial == "S"));
    }
}
```

- [ ] **Step 2: Update re-exports in `crates/fl-ios/src/lib.rs`**

```rust
//! Apple device discovery via `xcrun devicectl` and `xcrun simctl`.

pub mod parse;
pub mod watcher;
pub mod xcrun;

pub use parse::{parse_devicectl_json, parse_simctl_json};
pub use watcher::{diff_devices, list_apple_devices, watch_apple_devices};
pub use xcrun::Xcrun;
```

- [ ] **Step 3: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-ios`
Expected: 11 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/fl-ios/
git -c commit.gpgsign=false commit -m "feat(ios): watcher with list_apple_devices, diff_devices, watch_apple_devices

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Devices panel — render `platform` tag column

**Files:**
- Modify: `crates/fl-tui/src/panels/devices.rs`

- [ ] **Step 1: Update `lines_for` in `crates/fl-tui/src/panels/devices.rs`**

Find the `row1` construction. Add a platform span between the display name and the icon. The full updated `row1`:

```rust
    let plat_display = session.platform.as_deref().map(|p| if p == "ios-simulator" { "ios-sim" } else { p }).unwrap_or("");
    let row1 = Line::from(vec![
        Span::styled(format!("{bullet} "), Style::default().fg(bullet_color).bg(theme.bg)),
        Span::styled(format!("[{:<8}] ", session.short_name), Style::default().fg(prefix_color).bg(theme.bg)),
        Span::styled(session.display_name.clone(), theme.base()),
        Span::raw("  "),
        Span::styled(format!("{plat_display:<9}"), theme.dimmed()),
        Span::raw(" "),
        Span::styled(icon.to_string(), theme.dimmed()),
        Span::raw("  "),
        Span::styled(state_label.to_string(), theme.dimmed()),
    ]);
```

- [ ] **Step 2: Add a test asserting the platform tag appears**

Append to `mod tests`:

```rust
    #[test]
    fn render_includes_platform_tag() {
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(fl_core::AppEvent::Device(fl_core::DeviceEvent::SessionState {
            serial: "ABC".into(),
            state: fl_core::DeviceSessionState::Ready,
        }));
        // After SessionState, the platform field is None — feed Discovered to set it.
        s.apply(fl_core::AppEvent::Device(fl_core::DeviceEvent::Discovered(
            fl_core::Device {
                serial: "ABC".into(),
                name: "iPhone".into(),
                model: None,
                connection: fl_core::ConnectionKind::Wifi,
                state: fl_core::DeviceState::Online,
                ip: None,
                android_version: None,
                battery: None,
                platform: Some("ios".into()),
            }
        )));
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 6));
        render_devices(Rect::new(0, 0, 80, 6), &mut buf, &s, &Theme::TOKYO_NIGHT);
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width { text.push_str(buf.get(x, y).symbol()); }
            text.push('\n');
        }
        assert!(text.contains("ios"), "missing platform tag, got:\n{text}");
    }
```

- [ ] **Step 3: Run tests + accept snapshot if needed**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui`
Expected: pass. If `dashboard_snapshot` fails, accept:

`. "$HOME/.cargo/env" && INSTA_UPDATE=always cargo test -p fl-tui dashboard_snapshot 2>&1 | tail -5`

- [ ] **Step 4: Commit**

```bash
git add crates/fl-tui/
git -c commit.gpgsign=false commit -m "feat(tui): show platform tag in Devices panel rows

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Picker view — render `platform` tag column

**Files:**
- Modify: `crates/fl-tui/src/views/device_picker.rs`

- [ ] **Step 1: Update the `render` function in `device_picker.rs`**

Find the format string that produces each picker row. Replace it with a version including the platform tag.

The current line is approximately:
```rust
lines.push(Line::styled(
    format!("{arrow}{bullet} {:<22} {} · {}", d.name, conn, d.serial),
    if i == self.cursor { ... } else { theme.base() },
));
```

Replace with:
```rust
let plat = d.platform.as_deref().map(|p| if p == "ios-simulator" { "ios-sim" } else { p }).unwrap_or("");
lines.push(Line::styled(
    format!("{arrow}{bullet} {:<22} {:<9} {} · {}", d.name, plat, conn, d.serial),
    if i == self.cursor {
        Style::default().fg(theme.accent).bg(theme.bg)
    } else {
        theme.base()
    },
));
```

- [ ] **Step 2: Add a test asserting the platform column appears**

Append to `mod tests`:

```rust
    #[test]
    fn renders_platform_column() {
        use crate::theme::Theme;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let dev_with_platform = Device {
            serial: "X".into(),
            name: "iPhone".into(),
            model: None,
            connection: ConnectionKind::Usb,
            state: DeviceState::Online,
            ip: None,
            android_version: None,
            battery: None,
            platform: Some("ios".into()),
        };
        let v = DevicePickerView::with_devices(vec![dev_with_platform]);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 6));
        v.render(Rect::new(0, 0, 80, 6), &mut buf, &Theme::TOKYO_NIGHT);
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width { text.push_str(buf.get(x, y).symbol()); }
            text.push('\n');
        }
        assert!(text.contains("ios"), "missing platform tag, got:\n{text}");
    }
```

- [ ] **Step 3: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui`
Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/fl-tui/
git -c commit.gpgsign=false commit -m "feat(tui): show platform tag column in DevicePickerView rows

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Multi-device orchestrator integrates iOS

**Files:**
- Modify: `crates/fl-cli/Cargo.toml`
- Modify: `crates/fl-cli/src/multi.rs`

- [ ] **Step 1: Add `fl-ios` dependency to `crates/fl-cli/Cargo.toml`**

In `[dependencies]`:

```toml
fl-ios = { path = "../fl-ios" }
```

- [ ] **Step 2: Update `crates/fl-cli/src/multi.rs::run_multi`**

Locate the discovery block:
```rust
    let listed = runner.run("adb", &["devices", "-l"]).await?;
    let all_devices = parse_devices_l(&listed.stdout);
```

Replace with:
```rust
    let listed = runner.run("adb", &["devices", "-l"]).await?;
    let mut all_devices = parse_devices_l(&listed.stdout);
    let xcrun = fl_ios::Xcrun::new(TokioRunner);
    all_devices.extend(fl_ios::list_apple_devices(&xcrun).await);
```

> Note: we instantiate a fresh `TokioRunner` for the xcrun call rather than reusing `runner: Arc<TokioRunner>` because the `Xcrun` wrapper consumes the runner by value. `TokioRunner` is zero-sized so this is essentially free.

- [ ] **Step 3: Update the `usb_pair` selection in `run_multi` to be platform-aware**

Locate:
```rust
        let usb_pair = all_devices
            .iter()
            .find(|d| d.serial == *serial && matches!(d.connection, fl_core::ConnectionKind::Usb))
            .map(|d| d.serial.clone());
```

Replace with:
```rust
        let usb_pair = all_devices
            .iter()
            .find(|d| d.serial == *serial
                  && matches!(d.connection, fl_core::ConnectionKind::Usb)
                  && (d.platform.as_deref() == Some("android") || d.platform.is_none()))
            .map(|d| d.serial.clone());
```

- [ ] **Step 4: Spawn the iOS watcher alongside `track_devices`**

After the existing block that spawns `track_devices`, add:

```rust
    {
        let tx = event_tx.clone();
        tokio::spawn(async move {
            let xcrun = fl_ios::Xcrun::new(TokioRunner);
            fl_ios::watch_apple_devices(xcrun, tx).await;
        });
    }
```

- [ ] **Step 5: Build the workspace**

Run: `. "$HOME/.cargo/env" && cargo build --workspace 2>&1 | tail -10`
Expected: clean build.

- [ ] **Step 6: Run all tests + clippy**

Run: `. "$HOME/.cargo/env" && cargo test --workspace -- --test-threads=1 2>&1 | grep "test result"`
Expected: all `ok`.

Run: `. "$HOME/.cargo/env" && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/fl-cli/
git -c commit.gpgsign=false commit -m "feat(cli): multi.rs merges iOS devices and skips pre-pair for them

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Faux `xcrun` + fixtures

**Files:**
- Create: `tests/fixtures/bin/xcrun`
- Create: `tests/fixtures/scenarios/ios_one_device.json`
- Create: `tests/fixtures/scenarios/sim_one_booted.json`

- [ ] **Step 1: Create `tests/fixtures/bin/xcrun`**

```bash
#!/bin/sh
# Faux xcrun for fl integration tests.
# Routes:
#   xcrun devicectl list devices --json-output - -> $FL_XCRUN_DEVICECTL_SCENARIO
#   xcrun simctl list devices --json            -> $FL_XCRUN_SIMCTL_SCENARIO

cmd="$1"
sub="$2"
case "$cmd" in
  devicectl)
    if [ "$sub" = "list" ]; then
      if [ -n "$FL_XCRUN_DEVICECTL_SCENARIO" ] && [ -f "$FL_XCRUN_DEVICECTL_SCENARIO" ]; then
        cat "$FL_XCRUN_DEVICECTL_SCENARIO"
      else
        echo '{"result":{"devices":[]}}'
      fi
    fi
    ;;
  simctl)
    if [ "$sub" = "list" ]; then
      if [ -n "$FL_XCRUN_SIMCTL_SCENARIO" ] && [ -f "$FL_XCRUN_SIMCTL_SCENARIO" ]; then
        cat "$FL_XCRUN_SIMCTL_SCENARIO"
      else
        echo '{"devices":{}}'
      fi
    fi
    ;;
esac
exit 0
```

- [ ] **Step 2: Make it executable**

Run: `chmod +x tests/fixtures/bin/xcrun`

- [ ] **Step 3: Create `tests/fixtures/scenarios/ios_one_device.json`**

```json
{
  "result": {
    "devices": [
      {
        "identifier": "00008140-0011002233",
        "deviceProperties": {
          "name": "iPhone 15",
          "osVersionNumber": "17.4.1",
          "platform": "iOS"
        },
        "connectionProperties": {
          "transportType": "wired",
          "tunnelState": "connected"
        }
      }
    ]
  }
}
```

- [ ] **Step 4: Create `tests/fixtures/scenarios/sim_one_booted.json`**

```json
{
  "devices": {
    "com.apple.CoreSimulator.SimRuntime.iOS-17-4": [
      {
        "udid": "BOOTED-SIM-1111",
        "name": "iPhone 15 Pro Simulator",
        "state": "Booted",
        "isAvailable": true
      }
    ]
  }
}
```

- [ ] **Step 5: Sanity check**

Run:
```bash
FL_XCRUN_DEVICECTL_SCENARIO=tests/fixtures/scenarios/ios_one_device.json bash tests/fixtures/bin/xcrun devicectl list devices --json-output -
```
Expected: prints the JSON contents.

Run:
```bash
FL_XCRUN_SIMCTL_SCENARIO=tests/fixtures/scenarios/sim_one_booted.json bash tests/fixtures/bin/xcrun simctl list devices --json
```
Expected: prints the JSON contents.

- [ ] **Step 6: Commit**

```bash
git add tests/fixtures/
git -c commit.gpgsign=false commit -m "test: faux xcrun and iOS device/simulator JSON fixtures

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Headless iOS integration test

**Files:**
- Modify: `crates/fl-cli/tests/headless_run.rs`

- [ ] **Step 1: Append a new test to `crates/fl-cli/tests/headless_run.rs`**

```rust
#[test]
fn headless_ios_run_emits_app_started() {
    ensure_binary_built();
    let _ = std::fs::remove_dir_all("/tmp/fl-fake-adb");

    let pubspec = pubspec_in_workspace();
    let devicectl_scenario = fixtures().join("scenarios/ios_one_device.json");
    let flutter_scenario = fixtures().join("scenarios/nominal.txt");

    // Empty adb devices list — only iOS device present.
    let empty_adb = workspace_root().join("target/test-empty-adb-devices.txt");
    std::fs::write(&empty_adb, "List of devices attached\n").unwrap();

    let out = run_fl_with_env(
        &[
            "run",
            "--no-picker", "--no-wifi",
            "--device", "00008140-0011002233",
            "--project", pubspec.to_str().unwrap(),
        ],
        &[
            ("FL_ADB_FIXTURE_DEVICES", &empty_adb),
            ("FL_XCRUN_DEVICECTL_SCENARIO", &devicectl_scenario),
            ("FL_FLUTTER_SCENARIO", &flutter_scenario),
        ],
    );
    assert!(out.contains("AppStarted"), "missing AppStarted, output:\n{out}");
    // No pre-pair failure should ever fire for iOS — verify by absence.
    assert!(!out.contains("pre-pair failed"), "iOS device wrongly triggered pre-pair:\n{out}");
}
```

- [ ] **Step 2: Run the integration tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-cli --test headless_run -- --test-threads=1 2>&1 | tail -15`
Expected: 10 tests pass (9 prior + 1 new).

- [ ] **Step 3: Full workspace test + clippy**

Run: `. "$HOME/.cargo/env" && cargo test --workspace -- --test-threads=1 2>&1 | grep "test result"`
Expected: all `ok`.

Run: `. "$HOME/.cargo/env" && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/fl-cli/tests/headless_run.rs
git -c commit.gpgsign=false commit -m "test: headless iOS run does not trigger pre-pair

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage:**
- §3 fl-ios crate (xcrun.rs, parse.rs, watcher.rs) → Tasks 2, 3, 4, 5 ✓
- §4 Device.platform + DeviceSessionSummary.platform → Task 1 ✓
- §5 Devices panel platform column → Task 6 ✓
- §6 multi.rs discovery merge + iOS-aware usb_pair + watcher spawn → Task 8 ✓
- §7 Picker view platform column → Task 7 ✓
- §8 Error handling (xcrun absent, malformed JSON, Developer-Mode-disabled) → covered in Task 3 (`returns_empty` tests) and Task 5 (xcrun absent treated as empty list) ✓
- §9 Tests (parser fixtures, diff, integration) → Tasks 3, 4, 5, 10 ✓
- §10 File-level diff → all files covered ✓

**2. Placeholder scan:** No TBD/TODO/"similar to" patterns. Every step has executable content.

**3. Type consistency:**
- `Device.platform: Option<String>` introduced in Task 1, set by `parse_devices_l` (Task 1 Step 6), `parse_track_payload` (Task 1 Step 6), `parse_devicectl_json` (Task 3), `parse_simctl_json` (Task 4); read in `lines_for` (Task 6), `DevicePickerView::render` (Task 7), `run_multi` usb_pair (Task 8).
- `DeviceSessionSummary.platform: Option<String>` populated in `apply_device(SessionState)` (Task 1 Step 6) and copied from `Discovered` (Task 1 Step 6); read in Devices panel (Task 6).
- `Xcrun::devicectl_list` / `simctl_list` (Task 2) consumed by `list_apple_devices` (Task 5).
- `list_apple_devices` consumed by `run_multi` and `watch_apple_devices` (Tasks 5, 8).
- `parse_devicectl_json` / `parse_simctl_json` (Tasks 3, 4) consumed by `list_apple_devices` (Task 5).

---

## Execution Handoff

**Plan complete and saved to [docs/superpowers/plans/2026-05-18-ios-support.md](2026-05-18-ios-support.md). 10 TDD tasks total.**

**Two execution options:**

**1. Subagent-Driven (recommended)** — Fresh subagent per task.

**2. Inline Execution** — In-session with checkpoints.

**Which approach?**
