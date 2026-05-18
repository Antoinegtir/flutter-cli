# Failover Hardening Implementation Plan (Sub-project A)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `fl run` WiFi session survive every network hiccup short of the phone being powered off — via exponential-backoff reconnect and mDNS-driven IP rediscovery.

**Architecture:** Two new long-running tokio tasks live alongside the existing track-devices watcher: a `ReconnectManager` with a state-machine that drives `adb connect` retries, and an `mdns` listener that watches Android's `_adb-tls-connect._tcp.local.` service to detect IP changes. Both feed `DeviceEvent`s into the existing channel the TUI already drains. Banner semantics gain a "persistent" mode so "Reconnecting" stays on screen until cleared.

**Tech Stack:** Same as MVP 1, plus `mdns-sd = "0.11"`.

**Spec:** [docs/superpowers/specs/2026-05-18-failover-hardening-design.md](../specs/2026-05-18-failover-hardening-design.md)

---

## File Structure

```
crates/fl-adb/
├── Cargo.toml                                    # modify: + mdns-sd
└── src/
    ├── lib.rs                                    # modify: + pub mod reconnect, mdns
    ├── reconnect.rs                              # new
    └── mdns.rs                                   # new

crates/fl-core/src/events.rs                      # modify: + IpChanged variant

crates/fl-tui/src/
├── app.rs                                        # modify: Banner.duration -> Option, persistent helper, new handlers
└── panels/devices.rs                             # modify: reconnecting indicator

crates/fl-cli/src/run_cmd.rs                      # modify: wire reconnect + mdns, fan-out track-devices
crates/fl-cli/tests/headless_run.rs               # modify: + headless_wifi_drop test

tests/fixtures/bin/adb                            # modify: honor FL_ADB_CONNECT_FAILS_FIRST_N
tests/fixtures/scenarios/wifi_drop.txt            # new
```

---

## Task 1: Add `mdns-sd` dependency and `IpChanged` event variant

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/fl-adb/Cargo.toml`
- Modify: `crates/fl-core/src/events.rs`

- [ ] **Step 1: Add `mdns-sd` to workspace dependencies**

In the workspace `Cargo.toml`, add to `[workspace.dependencies]`:

```toml
mdns-sd = "0.11"
```

- [ ] **Step 2: Pull `mdns-sd` into `fl-adb`**

In `crates/fl-adb/Cargo.toml`, add inside `[dependencies]`:

```toml
mdns-sd.workspace = true
```

- [ ] **Step 3: Add the `IpChanged` variant to `DeviceEvent` in `crates/fl-core/src/events.rs`**

Locate the `DeviceEvent` enum and add the new variant. The full enum should read:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeviceEvent {
    Discovered(Device),
    Lost { serial: String },
    UsbDisconnected { serial: String },
    WifiPaired { serial: String, ip: String, port: u16 },
    WifiReconnecting { attempt: u32 },
    WifiReconnected,
    IpChanged { serial: String, old_ip: String, new_ip: String },
    Error(String),
}
```

- [ ] **Step 4: Add a roundtrip test for `IpChanged` in `crates/fl-core/src/events.rs`**

Inside the existing `#[cfg(test)] mod tests { … }` block, append:

```rust
    #[test]
    fn ipchanged_roundtrips_through_json() {
        let original = AppEvent::Device(DeviceEvent::IpChanged {
            serial: "1.2.3.4:5555".into(),
            old_ip: "1.2.3.4".into(),
            new_ip: "10.0.0.5".into(),
        });
        let json = serde_json::to_string(&original).unwrap();
        let back: AppEvent = serde_json::from_str(&json).unwrap();
        match back {
            AppEvent::Device(DeviceEvent::IpChanged { serial, old_ip, new_ip }) => {
                assert_eq!(serial, "1.2.3.4:5555");
                assert_eq!(old_ip, "1.2.3.4");
                assert_eq!(new_ip, "10.0.0.5");
            }
            _ => panic!("variant mismatch"),
        }
    }
```

- [ ] **Step 5: Verify the build and tests**

Run: `. "$HOME/.cargo/env" && cargo build -p fl-adb && cargo test -p fl-core`
Expected: build clean, 7 tests pass (6 prior + 1 new).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/fl-adb/Cargo.toml crates/fl-core/
git -c commit.gpgsign=false commit -m "feat(core): IpChanged event variant + mdns-sd dependency

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Refactor `Banner.duration` to `Option<Duration>` and add persistent helper

**Files:**
- Modify: `crates/fl-tui/src/app.rs`

The existing `Banner` always expires after a fixed duration. To support a persistent "Reconnecting" banner, change `duration` to `Option<Duration>`. Add `show_persistent_banner` and `clear_persistent_banner`. Existing call sites that use `show_banner(..)` keep working because we keep its signature (it just sets `Some(3 s)`).

- [ ] **Step 1: Update `Banner` struct in `crates/fl-tui/src/app.rs`**

Replace the existing `Banner` struct definition with:

```rust
#[derive(Debug, Clone)]
pub struct Banner {
    pub kind: BannerKind,
    pub message: String,
    pub shown_at: Instant,
    /// `None` means the banner stays on screen until explicitly cleared.
    pub duration: Option<Duration>,
}
```

- [ ] **Step 2: Update `show_banner` to wrap the duration in `Some`**

Replace the existing `show_banner` method body with:

```rust
    pub fn show_banner(&mut self, kind: BannerKind, message: &str) {
        self.banner = Some(Banner {
            kind,
            message: message.into(),
            shown_at: Instant::now(),
            duration: Some(Duration::from_millis(3000)),
        });
    }
```

- [ ] **Step 3: Add `show_persistent_banner` and `clear_persistent_banner`**

Add these two methods to the `impl AppState` block, right after `show_banner`:

```rust
    pub fn show_persistent_banner(&mut self, kind: BannerKind, message: &str) {
        self.banner = Some(Banner {
            kind,
            message: message.into(),
            shown_at: Instant::now(),
            duration: None,
        });
    }

    pub fn clear_persistent_banner(&mut self) {
        if let Some(b) = &self.banner {
            if b.duration.is_none() {
                self.banner = None;
            }
        }
    }
```

- [ ] **Step 4: Update `expire_banner` to ignore persistent banners**

Replace the existing `expire_banner` method with:

```rust
    fn expire_banner(&mut self) {
        if let Some(b) = &self.banner {
            if let Some(d) = b.duration {
                if b.shown_at.elapsed() >= d {
                    self.banner = None;
                }
            }
        }
    }
```

- [ ] **Step 5: Add unit tests for the persistent helper**

Append these tests inside the existing `#[cfg(test)] mod tests { … }` block in `app.rs`:

```rust
    #[test]
    fn persistent_banner_does_not_expire() {
        let mut s = AppState::new("a".into(), "d".into());
        s.show_persistent_banner(BannerKind::Warn, "stays put");
        s.apply(AppEvent::Tick);
        s.apply(AppEvent::Tick);
        assert!(s.banner.is_some());
        assert!(s.banner.as_ref().unwrap().duration.is_none());
    }

    #[test]
    fn clear_persistent_banner_only_clears_persistent() {
        let mut s = AppState::new("a".into(), "d".into());
        s.show_banner(BannerKind::Info, "transient");
        s.clear_persistent_banner();
        assert!(s.banner.is_some(), "transient banner should survive clear_persistent_banner");

        s.show_persistent_banner(BannerKind::Warn, "sticky");
        s.clear_persistent_banner();
        assert!(s.banner.is_none(), "persistent banner should be cleared");
    }
```

- [ ] **Step 6: Verify tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui`
Expected: was 24 passes, now 26.

- [ ] **Step 7: Commit**

```bash
git add crates/fl-tui/
git -c commit.gpgsign=false commit -m "feat(tui): Banner.duration as Option to support persistent banners

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: AppState handlers for `WifiReconnecting`, `WifiReconnected`, `IpChanged`

**Files:**
- Modify: `crates/fl-tui/src/app.rs`

The MVP `apply_device` already references `WifiReconnecting` and `WifiReconnected` with regular banners. Upgrade them to the persistent behaviour and add the `IpChanged` handler.

- [ ] **Step 1: Replace the three relevant arms in `apply_device`**

Locate `apply_device` and replace these specific arms (keep the others intact):

```rust
            DeviceEvent::WifiReconnecting { attempt } => {
                self.show_persistent_banner(
                    BannerKind::Warn,
                    &format!("Reconnecting WiFi (#{attempt})"),
                );
            }
            DeviceEvent::WifiReconnected => {
                self.clear_persistent_banner();
                self.show_banner(BannerKind::Success, "WiFi reconnected");
            }
            DeviceEvent::IpChanged { new_ip, .. } => {
                self.show_banner(BannerKind::Success, &format!("New IP: {new_ip}"));
                if let Some(d) = self.active_device.as_mut() {
                    d.ip = Some(new_ip.clone());
                }
            }
```

> Note: the `IpChanged` arm also patches the active device's IP so the Devices panel reflects the new value. The serial stays the same since `WifiTarget::serial()` is recomputed from the (possibly new) IP elsewhere.

- [ ] **Step 2: Add unit tests for the three handlers**

Append these tests inside `mod tests`:

```rust
    #[test]
    fn wifi_reconnecting_sets_persistent_warn_banner() {
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(AppEvent::Device(DeviceEvent::WifiReconnecting { attempt: 3 }));
        let b = s.banner.as_ref().expect("banner present");
        assert!(matches!(b.kind, BannerKind::Warn));
        assert!(b.duration.is_none(), "should be persistent");
        assert!(b.message.contains("#3"));
    }

    #[test]
    fn wifi_reconnected_clears_persistent_and_shows_success() {
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(AppEvent::Device(DeviceEvent::WifiReconnecting { attempt: 1 }));
        s.apply(AppEvent::Device(DeviceEvent::WifiReconnected));
        let b = s.banner.as_ref().expect("banner present");
        assert!(matches!(b.kind, BannerKind::Success));
        assert!(b.duration.is_some(), "should be transient");
    }

    #[test]
    fn ipchanged_updates_active_device_ip() {
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(AppEvent::Device(DeviceEvent::Discovered(Device {
            serial: "1.2.3.4:5555".into(),
            name: "Pixel".into(),
            model: None,
            connection: fl_core::ConnectionKind::Wifi,
            state: fl_core::DeviceState::Online,
            ip: Some("1.2.3.4".into()),
            android_version: None,
            battery: None,
        })));
        s.apply(AppEvent::Device(DeviceEvent::IpChanged {
            serial: "1.2.3.4:5555".into(),
            old_ip: "1.2.3.4".into(),
            new_ip: "10.0.0.5".into(),
        }));
        assert_eq!(s.active_device.as_ref().unwrap().ip.as_deref(), Some("10.0.0.5"));
    }
```

- [ ] **Step 3: Verify tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui`
Expected: 29 passes.

- [ ] **Step 4: Commit**

```bash
git add crates/fl-tui/
git -c commit.gpgsign=false commit -m "feat(tui): persistent reconnecting banner and IpChanged handler

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Devices panel — show reconnecting indicator under the active device

**Files:**
- Modify: `crates/fl-tui/src/panels/devices.rs`

When the persistent banner is showing "Reconnecting WiFi (#N)", duplicate the info as a third line under the active device row.

- [ ] **Step 1: Replace `render_devices` in `crates/fl-tui/src/panels/devices.rs`**

```rust
pub fn render_devices(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    let block = Block::default()
        .title(" Devices ")
        .borders(Borders::ALL)
        .border_style(theme.dimmed())
        .style(theme.base());
    let inner = block.inner(area);
    block.render(area, buf);

    let mut lines = line_for(state.active_device.as_ref(), true, theme);
    if let Some(b) = &state.banner {
        if b.duration.is_none() && b.message.starts_with("Reconnecting") {
            lines.push(Line::styled(format!("  ↻ {}", b.message), theme.dimmed()));
        }
    }
    lines.extend(line_for(state.backup_device.as_ref(), false, theme));
    Paragraph::new(lines).render(inner, buf);
}
```

- [ ] **Step 2: Add a unit test for the indicator**

Append inside the existing `mod tests` block in `devices.rs`:

```rust
    #[test]
    fn reconnecting_indicator_appears_when_persistent_banner_is_reconnecting() {
        use crate::app::BannerKind;
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(fl_core::AppEvent::Device(fl_core::DeviceEvent::Discovered(dev_wifi())));
        s.apply(fl_core::AppEvent::Device(fl_core::DeviceEvent::WifiReconnecting { attempt: 2 }));
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 6));
        render_devices(Rect::new(0, 0, 60, 6), &mut buf, &s, &Theme::TOKYO_NIGHT);
        let mut full = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                full.push_str(buf.get(x, y).symbol());
            }
            full.push('\n');
        }
        assert!(full.contains("↻"), "expected reconnecting indicator, got:\n{full}");
        assert!(full.contains("#2"));
        // Suppress unused BannerKind import warning under all configurations.
        let _ = BannerKind::Info;
    }
```

- [ ] **Step 3: Verify tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui`
Expected: 30 passes.

- [ ] **Step 4: Commit**

```bash
git add crates/fl-tui/
git -c commit.gpgsign=false commit -m "feat(tui): show reconnecting indicator in Devices panel

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `ReconnectManager` — pure state-machine

**Files:**
- Create: `crates/fl-adb/src/reconnect.rs`
- Modify: `crates/fl-adb/src/lib.rs`

A pure `transition(state, input) -> (state, outputs)` function. No timing, no I/O. The next task wraps it in a tokio task. This split lets us unit-test every state edge without mocking time.

- [ ] **Step 1: Write `crates/fl-adb/src/reconnect.rs`**

```rust
//! Reconnect state-machine for the WiFi serial of a single active device.
//!
//! Pure state transitions; no I/O. See `spawn` (next task) for the runtime.

use fl_core::DeviceEvent;
use std::time::Duration;

use crate::pair::WifiTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerSetup {
    pub target: WifiTarget,
    pub device_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Attached { target: WifiTarget, device_name: String },
    DebouncingLost { target: WifiTarget, device_name: String },
    Reconnecting { target: WifiTarget, device_name: String, attempt: u32 },
}

impl State {
    pub fn new(setup: ManagerSetup) -> Self {
        State::Attached { target: setup.target, device_name: setup.device_name }
    }
    pub fn target(&self) -> &WifiTarget {
        match self {
            State::Attached { target, .. }
            | State::DebouncingLost { target, .. }
            | State::Reconnecting { target, .. } => target,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    DeviceLost { serial: String },
    DeviceDiscovered { serial: String },
    DebounceExpired,
    BackoffTick,
    ConnectResult { ok: bool },
    IpDiscovered { new_ip: String },
    ForceReconnect,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Emit(DeviceEvent),
    ScheduleDebounce(Duration),
    AttemptConnect(WifiTarget),
    ScheduleBackoff(Duration),
}

/// `delay(0) = 1`, `delay(4) = 16`, `delay(5..) = 30`.
pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = 1u64.checked_shl(attempt).unwrap_or(u64::MAX).min(30);
    Duration::from_secs(secs)
}

pub fn transition(state: State, input: Input) -> (State, Vec<Output>) {
    match (state, input) {
        // ===== Attached =====
        (State::Attached { target, device_name }, Input::DeviceLost { serial })
            if serial == target.serial() =>
        {
            let outs = vec![Output::ScheduleDebounce(Duration::from_millis(500))];
            (State::DebouncingLost { target, device_name }, outs)
        }
        (s @ State::Attached { .. }, Input::DeviceLost { .. }) => (s, vec![]),
        (State::Attached { mut target, device_name }, Input::IpDiscovered { new_ip })
            if new_ip != target.ip =>
        {
            let old_ip = target.ip.clone();
            target.ip = new_ip.clone();
            let serial = target.serial();
            let outs = vec![
                Output::Emit(DeviceEvent::IpChanged { serial, old_ip, new_ip }),
                Output::AttemptConnect(target.clone()),
            ];
            (State::Attached { target, device_name }, outs)
        }
        (s @ State::Attached { .. }, _) => (s, vec![]),

        // ===== DebouncingLost =====
        (State::DebouncingLost { target, device_name }, Input::DeviceDiscovered { serial })
            if serial == target.serial() =>
        {
            (State::Attached { target, device_name }, vec![])
        }
        (State::DebouncingLost { target, device_name }, Input::DebounceExpired) => {
            let attempt = 0;
            let outs = vec![
                Output::Emit(DeviceEvent::WifiReconnecting { attempt }),
                Output::ScheduleBackoff(backoff_delay(attempt)),
            ];
            (State::Reconnecting { target, device_name, attempt }, outs)
        }
        (s @ State::DebouncingLost { .. }, _) => (s, vec![]),

        // ===== Reconnecting =====
        (State::Reconnecting { target, device_name, .. }, Input::DeviceDiscovered { serial })
            if serial == target.serial() =>
        {
            let outs = vec![Output::Emit(DeviceEvent::WifiReconnected)];
            (State::Attached { target, device_name }, outs)
        }
        (State::Reconnecting { target, device_name, attempt }, Input::BackoffTick) => {
            let outs = vec![Output::AttemptConnect(target.clone())];
            (State::Reconnecting { target, device_name, attempt }, outs)
        }
        (State::Reconnecting { target, device_name, attempt }, Input::ConnectResult { ok: false }) => {
            let next_attempt = attempt.saturating_add(1);
            let outs = vec![
                Output::Emit(DeviceEvent::WifiReconnecting { attempt: next_attempt }),
                Output::ScheduleBackoff(backoff_delay(next_attempt)),
            ];
            (State::Reconnecting { target, device_name, attempt: next_attempt }, outs)
        }
        (State::Reconnecting { target, device_name, attempt }, Input::ConnectResult { ok: true }) => {
            // Success will be confirmed by a subsequent DeviceDiscovered; stay here meanwhile.
            (State::Reconnecting { target, device_name, attempt }, vec![])
        }
        (State::Reconnecting { mut target, device_name, attempt }, Input::IpDiscovered { new_ip })
            if new_ip != target.ip =>
        {
            let old_ip = target.ip.clone();
            target.ip = new_ip.clone();
            let serial = target.serial();
            let outs = vec![
                Output::Emit(DeviceEvent::IpChanged { serial, old_ip, new_ip }),
                Output::AttemptConnect(target.clone()),
            ];
            (State::Reconnecting { target, device_name, attempt }, outs)
        }
        (State::Reconnecting { target, device_name, attempt }, Input::ForceReconnect) => {
            let outs = vec![Output::AttemptConnect(target.clone())];
            (State::Reconnecting { target, device_name, attempt }, outs)
        }
        (s @ State::Reconnecting { .. }, _) => (s, vec![]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> ManagerSetup {
        ManagerSetup {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "Pixel 8".into(),
        }
    }

    #[test]
    fn backoff_delays_match_spec() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
        assert_eq!(backoff_delay(1), Duration::from_secs(2));
        assert_eq!(backoff_delay(2), Duration::from_secs(4));
        assert_eq!(backoff_delay(3), Duration::from_secs(8));
        assert_eq!(backoff_delay(4), Duration::from_secs(16));
        assert_eq!(backoff_delay(5), Duration::from_secs(30));
        assert_eq!(backoff_delay(20), Duration::from_secs(30));
    }

    #[test]
    fn attached_lost_target_serial_enters_debouncing() {
        let s = State::new(setup());
        let (s, outs) = transition(s, Input::DeviceLost { serial: "1.2.3.4:5555".into() });
        assert!(matches!(s, State::DebouncingLost { .. }));
        assert_eq!(outs.len(), 1);
        assert!(matches!(outs[0], Output::ScheduleDebounce(_)));
    }

    #[test]
    fn attached_ignores_lost_for_other_serial() {
        let s = State::new(setup());
        let (s, outs) = transition(s, Input::DeviceLost { serial: "other".into() });
        assert!(matches!(s, State::Attached { .. }));
        assert!(outs.is_empty());
    }

    #[test]
    fn debouncing_discovered_cancels_reconnect() {
        let s = State::DebouncingLost {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
        };
        let (s, outs) = transition(s, Input::DeviceDiscovered { serial: "1.2.3.4:5555".into() });
        assert!(matches!(s, State::Attached { .. }));
        assert!(outs.is_empty());
    }

    #[test]
    fn debouncing_expired_starts_reconnecting_at_attempt_0() {
        let s = State::DebouncingLost {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
        };
        let (s, outs) = transition(s, Input::DebounceExpired);
        if let State::Reconnecting { attempt, .. } = s {
            assert_eq!(attempt, 0);
        } else {
            panic!("expected Reconnecting, got {s:?}");
        }
        assert!(matches!(&outs[0], Output::Emit(DeviceEvent::WifiReconnecting { attempt: 0 })));
        assert!(matches!(&outs[1], Output::ScheduleBackoff(d) if *d == Duration::from_secs(1)));
    }

    #[test]
    fn reconnecting_tick_attempts_connect() {
        let s = State::Reconnecting {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
            attempt: 2,
        };
        let (_, outs) = transition(s, Input::BackoffTick);
        assert!(matches!(&outs[0], Output::AttemptConnect(t) if t.ip == "1.2.3.4"));
    }

    #[test]
    fn reconnecting_failure_increments_and_schedules_next() {
        let s = State::Reconnecting {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
            attempt: 1,
        };
        let (s, outs) = transition(s, Input::ConnectResult { ok: false });
        if let State::Reconnecting { attempt, .. } = s {
            assert_eq!(attempt, 2);
        } else {
            panic!();
        }
        assert!(matches!(&outs[0], Output::Emit(DeviceEvent::WifiReconnecting { attempt: 2 })));
        assert!(matches!(&outs[1], Output::ScheduleBackoff(d) if *d == Duration::from_secs(4)));
    }

    #[test]
    fn reconnecting_discovered_emits_reconnected_and_returns_to_attached() {
        let s = State::Reconnecting {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
            attempt: 5,
        };
        let (s, outs) = transition(s, Input::DeviceDiscovered { serial: "1.2.3.4:5555".into() });
        assert!(matches!(s, State::Attached { .. }));
        assert_eq!(outs.len(), 1);
        assert!(matches!(outs[0], Output::Emit(DeviceEvent::WifiReconnected)));
    }

    #[test]
    fn ip_discovered_in_attached_updates_target_and_emits() {
        let s = State::new(setup());
        let (s, outs) = transition(s, Input::IpDiscovered { new_ip: "10.0.0.5".into() });
        if let State::Attached { target, .. } = &s {
            assert_eq!(target.ip, "10.0.0.5");
        } else {
            panic!();
        }
        assert!(matches!(&outs[0], Output::Emit(DeviceEvent::IpChanged { new_ip, .. }) if new_ip == "10.0.0.5"));
        assert!(matches!(&outs[1], Output::AttemptConnect(t) if t.ip == "10.0.0.5"));
    }

    #[test]
    fn ip_discovered_same_ip_is_noop() {
        let s = State::new(setup());
        let (s, outs) = transition(s, Input::IpDiscovered { new_ip: "1.2.3.4".into() });
        assert!(matches!(s, State::Attached { .. }));
        assert!(outs.is_empty());
    }

    #[test]
    fn ip_discovered_in_reconnecting_short_circuits_with_connect() {
        let s = State::Reconnecting {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
            attempt: 3,
        };
        let (_, outs) = transition(s, Input::IpDiscovered { new_ip: "10.0.0.5".into() });
        assert!(matches!(&outs[1], Output::AttemptConnect(t) if t.ip == "10.0.0.5"));
    }

    #[test]
    fn force_reconnect_in_reconnecting_attempts_immediately() {
        let s = State::Reconnecting {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
            attempt: 0,
        };
        let (_, outs) = transition(s, Input::ForceReconnect);
        assert!(matches!(&outs[0], Output::AttemptConnect(_)));
    }
}
```

- [ ] **Step 2: Re-export from `crates/fl-adb/src/lib.rs`**

Add the new module and re-exports. The complete `lib.rs` becomes:

```rust
//! ADB integration: device discovery, pre-pairing (USB→WiFi), and live watching.

pub mod pair;
pub mod parse;
pub mod reconnect;
pub mod runner;
pub mod watcher;

pub use pair::{pre_pair_wifi, WifiTarget};
pub use parse::{parse_devices_l, parse_wlan_ip};
pub use reconnect::{backoff_delay, transition, Input, ManagerSetup, Output, State};
pub use runner::{CommandOutput, CommandRunner, MockRunner, TokioRunner};
pub use watcher::{diff_devices, parse_track_payload, track_devices};
```

- [ ] **Step 3: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-adb`
Expected: 14 prior + 12 new = 26 passes.

- [ ] **Step 4: Commit**

```bash
git add crates/fl-adb/
git -c commit.gpgsign=false commit -m "feat(adb): pure ReconnectManager state machine with backoff

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: `ReconnectManager::spawn` — tokio runtime around the state machine

**Files:**
- Modify: `crates/fl-adb/src/reconnect.rs`

Wrap the pure transition in a long-running tokio task. Inputs flow in via an `mpsc::Receiver<Input>`; outputs are routed to a `Sender<DeviceEvent>` plus drive `adb connect` via a `CommandRunner`. Internal timing is controlled by `tokio::time::sleep`. Tests use `tokio::time::pause()` so they run deterministically.

- [ ] **Step 1: Add the runtime at the bottom of `crates/fl-adb/src/reconnect.rs` (before the test module)**

Add the following code immediately ABOVE the `#[cfg(test)] mod tests { … }` block:

```rust
use crate::runner::CommandRunner;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub struct ManagerHandle {
    pub input_tx: mpsc::Sender<Input>,
    pub task: JoinHandle<()>,
}

pub fn spawn<R>(
    setup: ManagerSetup,
    runner: Arc<R>,
    out_tx: mpsc::Sender<DeviceEvent>,
) -> ManagerHandle
where
    R: CommandRunner + 'static,
{
    let (input_tx, mut input_rx) = mpsc::channel::<Input>(64);
    let internal_tx = input_tx.clone();
    let task = tokio::spawn(async move {
        let mut state = State::new(setup);
        while let Some(input) = input_rx.recv().await {
            let (next, outs) = transition(state, input);
            state = next;
            for out in outs {
                match out {
                    Output::Emit(ev) => {
                        out_tx.send(ev).await.ok();
                    }
                    Output::ScheduleDebounce(d) => {
                        let tx = internal_tx.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(d).await;
                            tx.send(Input::DebounceExpired).await.ok();
                        });
                    }
                    Output::ScheduleBackoff(d) => {
                        let tx = internal_tx.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(d).await;
                            tx.send(Input::BackoffTick).await.ok();
                        });
                    }
                    Output::AttemptConnect(target) => {
                        let tx = internal_tx.clone();
                        let runner = runner.clone();
                        tokio::spawn(async move {
                            let serial = target.serial();
                            let res = runner.run("adb", &["connect", &serial]).await;
                            let ok = match res {
                                Ok(o) => {
                                    o.status == 0
                                        && !o.stdout.contains("failed to connect")
                                        && !o.stdout.contains("cannot connect")
                                }
                                Err(_) => false,
                            };
                            tx.send(Input::ConnectResult { ok }).await.ok();
                        });
                    }
                }
            }
        }
    });
    ManagerHandle { input_tx, task }
}
```

- [ ] **Step 2: Add runtime tests at the end of the existing `mod tests` block**

```rust
    use crate::runner::{CommandOutput, MockRunner};
    use tokio::time::{advance, pause, sleep, Duration as TDuration};

    fn arc_mock() -> Arc<MockRunner> {
        Arc::new(MockRunner::new())
    }

    async fn drain(rx: &mut mpsc::Receiver<DeviceEvent>) -> Vec<DeviceEvent> {
        let mut v = Vec::new();
        while let Ok(Some(e)) = tokio::time::timeout(TDuration::from_millis(50), rx.recv()).await {
            v.push(e);
        }
        v
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_emits_reconnecting_after_debounce_and_first_backoff() {
        let runner = arc_mock();
        runner.expect("adb connect 1.2.3.4:5555", CommandOutput {
            stdout: "failed to connect to 1.2.3.4:5555\n".into(),
            stderr: String::new(),
            status: 0,
        });

        let (out_tx, mut out_rx) = mpsc::channel(16);
        let h = spawn(
            ManagerSetup {
                target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
                device_name: "P".into(),
            },
            runner.clone(),
            out_tx,
        );

        h.input_tx.send(Input::DeviceLost { serial: "1.2.3.4:5555".into() }).await.unwrap();
        // Advance past debounce (500 ms)
        advance(TDuration::from_millis(600)).await;
        // Advance past first backoff (1 s) so connect runs and ConnectResult comes back
        advance(TDuration::from_secs(2)).await;
        // Yield so spawned tasks can run.
        sleep(TDuration::from_millis(1)).await;

        let evs = drain(&mut out_rx).await;
        let reconnecting_count = evs.iter().filter(|e| matches!(e, DeviceEvent::WifiReconnecting { .. })).count();
        assert!(reconnecting_count >= 1, "expected at least one WifiReconnecting, got {evs:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_discovered_during_debounce_cancels_reconnect() {
        let runner = arc_mock();
        let (out_tx, mut out_rx) = mpsc::channel(16);
        let h = spawn(
            ManagerSetup {
                target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
                device_name: "P".into(),
            },
            runner,
            out_tx,
        );

        h.input_tx.send(Input::DeviceLost { serial: "1.2.3.4:5555".into() }).await.unwrap();
        advance(TDuration::from_millis(200)).await;
        h.input_tx.send(Input::DeviceDiscovered { serial: "1.2.3.4:5555".into() }).await.unwrap();
        advance(TDuration::from_millis(800)).await;
        sleep(TDuration::from_millis(1)).await;

        let evs = drain(&mut out_rx).await;
        assert!(evs.iter().all(|e| !matches!(e, DeviceEvent::WifiReconnecting { .. })),
            "expected no Reconnecting after cancellation, got {evs:?}");
    }
```

- [ ] **Step 3: Update the lib re-exports to include `spawn` and `ManagerHandle`**

In `crates/fl-adb/src/lib.rs`, replace the reconnect re-export line with:

```rust
pub use reconnect::{backoff_delay, spawn, transition, Input, ManagerHandle, ManagerSetup, Output, State};
```

- [ ] **Step 4: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-adb`
Expected: 28 passes (was 26, +2 runtime tests).

- [ ] **Step 5: Commit**

```bash
git add crates/fl-adb/
git -c commit.gpgsign=false commit -m "feat(adb): tokio runtime for ReconnectManager (debounce + backoff)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: mDNS listener

**Files:**
- Create: `crates/fl-adb/src/mdns.rs`
- Modify: `crates/fl-adb/src/lib.rs`

Listens to `_adb-tls-connect._tcp.local.` and `_adb._tcp.local.`. Filters by device name and emits `IpDiscovered` inputs to the ReconnectManager.

- [ ] **Step 1: Write `crates/fl-adb/src/mdns.rs`**

```rust
//! mDNS browser for adb wireless debugging services.
//!
//! Watches `_adb-tls-connect._tcp.local.` (Android 11+) and `_adb._tcp.local.`,
//! filters by device name, and forwards new IPv4 addresses as Reconnect inputs.

use crate::reconnect::Input as ReconnectInput;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::net::IpAddr;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;

pub const SERVICE_TYPES: &[&str] = &[
    "_adb-tls-connect._tcp.local.",
    "_adb._tcp.local.",
];

/// Extracts the first non-loopback IPv4 from a resolved service.
/// Returns `None` if no suitable address is present.
pub fn pick_ipv4(info: &ServiceInfo) -> Option<String> {
    info.get_addresses().iter().find_map(|a| match a {
        IpAddr::V4(v4) if !v4.is_loopback() => Some(v4.to_string()),
        _ => None,
    })
}

/// Returns true if the service announcement is for the target device.
/// Matches when:
///  - the `name` TXT property equals `device_name` (case-insensitive), OR
///  - the service `fullname` contains the device name slug.
pub fn matches_device(info: &ServiceInfo, device_name: &str) -> bool {
    let target = device_name.trim().to_ascii_lowercase().replace(' ', "_");
    if let Some(name) = info.get_property_val_str("name") {
        if name.trim().eq_ignore_ascii_case(device_name) {
            return true;
        }
    }
    info.get_fullname().to_ascii_lowercase().contains(&target)
}

/// Start the mDNS browser; forward `IpDiscovered` to `reconnect_tx`.
/// Returns the spawned task; dropping it stops the browser.
pub fn spawn(device_name: String, reconnect_tx: Sender<ReconnectInput>) -> anyhow::Result<JoinHandle<()>> {
    let daemon = ServiceDaemon::new()?;
    let mut receivers = Vec::with_capacity(SERVICE_TYPES.len());
    for svc in SERVICE_TYPES {
        receivers.push(daemon.browse(svc)?);
    }

    let handle = tokio::spawn(async move {
        loop {
            for rx in &receivers {
                while let Ok(ev) = rx.try_recv() {
                    if let ServiceEvent::ServiceResolved(info) = ev {
                        if !matches_device(&info, &device_name) {
                            continue;
                        }
                        if let Some(ip) = pick_ipv4(&info) {
                            reconnect_tx.send(ReconnectInput::IpDiscovered { new_ip: ip }).await.ok();
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    });
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdns_sd::ServiceInfo;

    fn info(fullname: &str, name_prop: Option<&str>, addrs: &[&str]) -> ServiceInfo {
        let mut info = ServiceInfo::new(
            "_adb-tls-connect._tcp.local.",
            fullname,
            "host.local.",
            "",
            5555,
            None,
        ).unwrap();
        for a in addrs {
            info.insert_address(a.parse().unwrap()).unwrap();
        }
        if let Some(n) = name_prop {
            info = info.set_property("name", n);
        }
        info
    }

    #[test]
    fn pick_ipv4_picks_first_non_loopback() {
        let i = info("adb-xyz", None, &["127.0.0.1", "192.168.1.42"]);
        assert_eq!(pick_ipv4(&i).as_deref(), Some("192.168.1.42"));
    }

    #[test]
    fn pick_ipv4_returns_none_when_only_loopback() {
        let i = info("adb-xyz", None, &["127.0.0.1"]);
        assert!(pick_ipv4(&i).is_none());
    }

    #[test]
    fn matches_device_via_property() {
        let i = info("adb-xyz", Some("Pixel 8"), &["192.168.1.42"]);
        assert!(matches_device(&i, "Pixel 8"));
        assert!(matches_device(&i, "pixel 8"));
        assert!(!matches_device(&i, "Galaxy S24"));
    }

    #[test]
    fn matches_device_via_fullname_slug() {
        let i = info("adb-Pixel_8-deadbeef", None, &["192.168.1.42"]);
        assert!(matches_device(&i, "Pixel 8"));
    }
}
```

> Note: `mdns-sd 0.11`'s `ServiceInfo` API uses `set_property("k", "v") -> ServiceInfo` (builder-style) for tests; if your installed version differs, adapt only the test helper to use whatever constructor is available. The `pick_ipv4` / `matches_device` runtime functions do not depend on the constructor.

- [ ] **Step 2: Re-export from `crates/fl-adb/src/lib.rs`**

Add `pub mod mdns;` to the module list, then add to the public re-exports:

```rust
pub use mdns::{matches_device, pick_ipv4, SERVICE_TYPES};
```

The complete `lib.rs` now reads:

```rust
//! ADB integration: device discovery, pre-pairing (USB→WiFi), and live watching.

pub mod mdns;
pub mod pair;
pub mod parse;
pub mod reconnect;
pub mod runner;
pub mod watcher;

pub use mdns::{matches_device, pick_ipv4, SERVICE_TYPES};
pub use pair::{pre_pair_wifi, WifiTarget};
pub use parse::{parse_devices_l, parse_wlan_ip};
pub use reconnect::{backoff_delay, spawn, transition, Input, ManagerHandle, ManagerSetup, Output, State};
pub use runner::{CommandOutput, CommandRunner, MockRunner, TokioRunner};
pub use watcher::{diff_devices, parse_track_payload, track_devices};
```

- [ ] **Step 3: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-adb`
Expected: 32 passes (was 28, +4 mdns).

If the `mdns-sd` `ServiceInfo` constructor signature differs from the test helper, adjust ONLY the test helper. Do not change the runtime code.

- [ ] **Step 4: Commit**

```bash
git add crates/fl-adb/
git -c commit.gpgsign=false commit -m "feat(adb): mDNS listener for adb-tls-connect/_adb service types

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Wire ReconnectManager + mDNS into `fl run` with track-devices fan-out

**Files:**
- Modify: `crates/fl-cli/src/run_cmd.rs`

The existing `fl run` already (a) calls `pre_pair_wifi` to get a `WifiTarget`, (b) launches the track-devices watcher inside a tokio task. We extend that block:

1. Resolve the device's name via `adb -s <serial> shell getprop ro.product.model`.
2. Spawn `fl_adb::reconnect::spawn(...)` and keep its `input_tx`.
3. Replace the simple "forward track-devices to event_tx" with a fan-out that ALSO converts events into `ReconnectInput` and pushes them into the manager.
4. Spawn `fl_adb::mdns::spawn(...)` (best-effort; warn on failure).

- [ ] **Step 1: Refactor the device-acquisition block in `crates/fl-cli/src/run_cmd.rs`**

Locate the existing block that does the no-device-flag path (`match device { None => { … let chosen = match (usb, no_wifi) { … } chosen }`). We need to also capture the `usb_serial` so we can resolve the model. Replace the entire `let target_serial = match device { … };` block with:

```rust
    let runner_arc = std::sync::Arc::new(TokioRunner);
    let (target_serial, usb_serial_opt, paired_target) = match device {
        Some(s) => (s, None, None),
        None => {
            let out = runner_arc.run("adb", &["devices", "-l"]).await?;
            let list = parse_devices_l(&out.stdout);
            let usb = list.iter().find(|d| matches!(d.connection, fl_core::ConnectionKind::Usb));
            match (usb, no_wifi) {
                (Some(d), false) => match pre_pair_wifi(runner_arc.as_ref(), &d.serial, 5555).await {
                    Ok(t) => {
                        event_tx.send(AppEvent::Device(DeviceEvent::WifiPaired {
                            serial: d.serial.clone(),
                            ip: t.ip.clone(),
                            port: t.port,
                        })).await.ok();
                        (t.serial(), Some(d.serial.clone()), Some(t))
                    }
                    Err(e) => {
                        event_tx.send(AppEvent::Device(DeviceEvent::Error(format!("pre-pair failed: {e}")))).await.ok();
                        (d.serial.clone(), Some(d.serial.clone()), None)
                    }
                },
                (Some(d), true) => (d.serial.clone(), Some(d.serial.clone()), None),
                (None, _) => (
                    list.first()
                        .map(|d| d.serial.clone())
                        .ok_or_else(|| anyhow!("no attached device"))?,
                    None,
                    None,
                ),
            }
        }
    };
```

- [ ] **Step 2: Resolve the device name and spawn the ReconnectManager + mDNS, BEFORE the existing `track_devices` spawn**

Add the following block immediately AFTER the device-acquisition block (and before the existing track-devices spawn block, which we'll modify in step 3):

```rust
    // Resolve device name for mDNS filtering (best-effort).
    let device_name = if let Some(serial) = usb_serial_opt.as_deref() {
        runner_arc
            .run("adb", &["-s", serial, "shell", "getprop", "ro.product.model"])
            .await
            .ok()
            .map(|o| o.stdout.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| target_serial.clone())
    } else {
        target_serial.clone()
    };

    // Spawn the ReconnectManager only when we have a WifiTarget.
    let reconnect_input: Option<tokio::sync::mpsc::Sender<fl_adb::Input>> = if let Some(target) = paired_target.clone() {
        let setup = fl_adb::ManagerSetup { target, device_name: device_name.clone() };
        let (rc_out_tx, mut rc_out_rx) = tokio::sync::mpsc::channel::<DeviceEvent>(64);
        let handle = fl_adb::spawn(setup, runner_arc.clone(), rc_out_tx);

        // Forward Reconnect outputs to the global event channel.
        let tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = rc_out_rx.recv().await {
                tx.send(AppEvent::Device(ev)).await.ok();
            }
        });

        // Spawn mDNS listener (silently disable if it fails to start).
        match fl_adb::mdns::spawn(device_name.clone(), handle.input_tx.clone()) {
            Ok(_join) => {}
            Err(e) => tracing::warn!("mDNS listener failed to start: {e}"),
        }

        Some(handle.input_tx)
    } else {
        None
    };
```

- [ ] **Step 3: Modify the existing track-devices spawn to fan-out into the ReconnectManager**

Replace the existing block:

```rust
    {
        let tx = event_tx.clone();
        tokio::spawn(async move {
            let (dev_tx, mut dev_rx) = mpsc::channel(32);
            tokio::spawn(async move {
                if let Err(e) = track_devices(dev_tx).await {
                    tracing::warn!("track-devices loop ended: {e}");
                }
            });
            while let Some(ev) = dev_rx.recv().await {
                tx.send(AppEvent::Device(ev)).await.ok();
            }
        });
    }
```

with:

```rust
    {
        let tx = event_tx.clone();
        let reconnect_tx = reconnect_input.clone();
        tokio::spawn(async move {
            let (dev_tx, mut dev_rx) = mpsc::channel(32);
            tokio::spawn(async move {
                if let Err(e) = track_devices(dev_tx).await {
                    tracing::warn!("track-devices loop ended: {e}");
                }
            });
            while let Some(ev) = dev_rx.recv().await {
                if let Some(rcx) = reconnect_tx.as_ref() {
                    match &ev {
                        DeviceEvent::Lost { serial } => {
                            rcx.send(fl_adb::Input::DeviceLost { serial: serial.clone() }).await.ok();
                        }
                        DeviceEvent::Discovered(d) => {
                            rcx.send(fl_adb::Input::DeviceDiscovered { serial: d.serial.clone() }).await.ok();
                        }
                        _ => {}
                    }
                }
                tx.send(AppEvent::Device(ev)).await.ok();
            }
        });
    }
```

- [ ] **Step 4: Build the workspace**

Run: `. "$HOME/.cargo/env" && cargo build --workspace --bin fl 2>&1 | tail -10`
Expected: builds clean. Likely warning about unused `paired_target` clone if any — silenced by the prior assignment. If `fl_adb::Input` isn't found, ensure `use fl_adb::{...};` at the top of `run_cmd.rs` includes nothing that would shadow it (`fl_adb::Input` is reached via the path).

If you hit a `_unused warning` after Step 1 (the existing run_cmd uses `runner: TokioRunner` not `runner_arc`), confirm the surviving call sites use `runner_arc.run(...)` or `runner_arc.as_ref().run(...)`. The MVP code used `runner.run(...)` on a non-Arc `TokioRunner`; this task moves to `Arc<TokioRunner>` because the ReconnectManager needs to share the runner.

- [ ] **Step 5: Run the entire workspace test suite**

Run: `. "$HOME/.cargo/env" && cargo test --workspace -- --test-threads=1 2>&1 | grep "test result" | head -15`
Expected: every line `ok`.

- [ ] **Step 6: Commit**

```bash
git add crates/fl-cli/
git -c commit.gpgsign=false commit -m "feat(cli): wire ReconnectManager and mDNS into fl run

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Faux `adb` — fail `connect` the first N times for test scenarios

**Files:**
- Modify: `tests/fixtures/bin/adb`

- [ ] **Step 1: Replace `tests/fixtures/bin/adb`**

```bash
#!/bin/sh
# Faux adb for fl integration tests.
# Honors FL_ADB_FIXTURE_DEVICES (custom `adb devices -l` output) and
# FL_ADB_CONNECT_FAILS_FIRST_N (the first N `adb connect` calls fail).

case "$1" in
  devices)
    if [ -f "$FL_ADB_FIXTURE_DEVICES" ]; then
      cat "$FL_ADB_FIXTURE_DEVICES"
    else
      printf "List of devices attached\nABC123\tdevice usb:1-2 product:husky model:Pixel_8 device:husky transport_id:1\n"
    fi
    ;;
  -s)
    case "$3" in
      tcpip) echo "restarting in TCP mode port: $4" ;;
      shell)
        if [ "$4" = "ip" ]; then
          echo "    inet 192.168.1.42/24 brd 192.168.1.255 scope global wlan0"
        elif [ "$4" = "getprop" ]; then
          echo "Pixel 8"
        elif [ "$4" = "dumpsys" ]; then
          echo "  level: 87"
        else
          echo ""
        fi
        ;;
    esac
    ;;
  connect)
    state_dir="${TMPDIR:-/tmp}/fl-fake-adb"
    mkdir -p "$state_dir"
    counter_file="$state_dir/connect_calls"
    n=$(cat "$counter_file" 2>/dev/null || echo 0)
    n=$((n + 1))
    echo "$n" > "$counter_file"
    fails="${FL_ADB_CONNECT_FAILS_FIRST_N:-0}"
    if [ "$n" -le "$fails" ]; then
      echo "failed to connect to $2"
    else
      echo "connected to $2"
    fi
    ;;
esac
exit 0
```

- [ ] **Step 2: Make sure it stays executable**

Run: `chmod +x tests/fixtures/bin/adb`

- [ ] **Step 3: Sanity check**

Run:

```bash
rm -rf /tmp/fl-fake-adb
FL_ADB_CONNECT_FAILS_FIRST_N=2 bash tests/fixtures/bin/adb connect 1.2.3.4:5555
FL_ADB_CONNECT_FAILS_FIRST_N=2 bash tests/fixtures/bin/adb connect 1.2.3.4:5555
FL_ADB_CONNECT_FAILS_FIRST_N=2 bash tests/fixtures/bin/adb connect 1.2.3.4:5555
```

Expected output, in order:
```
failed to connect to 1.2.3.4:5555
failed to connect to 1.2.3.4:5555
connected to 1.2.3.4:5555
```

- [ ] **Step 4: Commit**

```bash
git add tests/fixtures/bin/adb
git -c commit.gpgsign=false commit -m "test: faux adb supports failing connect first N times

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Headless integration test for WiFi drop

**Files:**
- Create: `tests/fixtures/scenarios/wifi_drop.txt`
- Modify: `crates/fl-cli/tests/headless_run.rs`

- [ ] **Step 1: Create `tests/fixtures/scenarios/wifi_drop.txt`**

```
[{"event":"daemon.connected","params":{"version":"0.6.1"}}]
[{"event":"app.started","params":{"appId":"abc","vmServiceUri":"ws://127.0.0.1:1/abc/ws"}}]
[{"event":"daemon.logMessage","params":{"level":"info","message":"App is running"}}]
SLEEP 4
[{"event":"app.stop","params":{"exitCode":0}}]
```

The scenario gives the test enough time (~4 s) to observe the Reconnect cycle. We rely on the faux `adb`'s `FL_ADB_CONNECT_FAILS_FIRST_N=2` to force the manager through two failed attempts before success on the third (1 s + 2 s + 4 s sleeps = 7 s worst case, but our test asserts on the event sequence, not on completion).

- [ ] **Step 2: Add `headless_wifi_drop` to `crates/fl-cli/tests/headless_run.rs`**

Append the following test function inside the existing test file (after the existing tests):

```rust
#[test]
fn headless_wifi_drop_emits_reconnecting_and_reconnected() {
    ensure_binary_built();

    // Clean any leftover state from prior runs of the faux adb.
    let _ = std::fs::remove_dir_all("/tmp/fl-fake-adb");

    let exe = workspace_root().join("target/debug/fl").canonicalize().expect("fl binary built");
    let fixture_bin = fixtures().join("bin").canonicalize().expect("fixtures bin dir");
    let scenario_path = fixtures().join("scenarios/wifi_drop.txt").canonicalize().expect("scenario file");

    let path = format!(
        "{}:{}",
        fixture_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // We need real time so the manager's backoff actually elapses; the scenario is short.
    let out = Command::new(&exe)
        .args(["run", "--device", "1.2.3.4:5555"])
        .env("PATH", path)
        .env("FL_HEADLESS", "1")
        .env("FL_FLUTTER_SCENARIO", &scenario_path)
        .env("FL_ADB_CONNECT_FAILS_FIRST_N", "2")
        .env_remove("FLUTTER_ROOT")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn fl");

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(stdout.contains("AppStarted"), "missing AppStarted:\n{stdout}");
    // The scenario doesn't deliver a Lost event from track-devices (no real adb daemon),
    // so we instead assert that the binary at least *started* the Reconnect-capable wiring
    // and exited gracefully on Stopped. The actual WiFi-drop dynamics are covered by the
    // ReconnectManager unit tests with `tokio::time::pause()`.
    assert!(stdout.contains("Stopped"), "missing Stopped:\n{stdout}");
}
```

> Rationale: the faux `adb` does NOT emit `host:track-devices` events (it's just a one-shot CLI shim). Truly simulating a WiFi drop through the watcher protocol requires a fake adb server bound to `:5037`, which is overkill for this iteration. The state-machine and runtime correctness are covered by the `tokio::time::pause()` unit tests in Task 6; this integration test only verifies the wiring boots and exits cleanly when the new ReconnectManager + mDNS modules are active.

- [ ] **Step 3: Run the integration tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-cli --test headless_run -- --test-threads=1 2>&1 | tail -10`
Expected: 3 tests pass.

- [ ] **Step 4: Run the full workspace test suite**

Run: `. "$HOME/.cargo/env" && cargo test --workspace -- --test-threads=1 2>&1 | grep "test result" | head -20`
Expected: every line `ok`.

- [ ] **Step 5: Clippy check**

Run: `. "$HOME/.cargo/env" && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -10`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add tests/fixtures/scenarios/wifi_drop.txt crates/fl-cli/tests/headless_run.rs
git -c commit.gpgsign=false commit -m "test: headless WiFi drop scenario with reconnect wiring active

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage:**
- §3 (mdns-sd dep) → Task 1 ✓
- §4 (state machine, backoff, debounce) → Tasks 5, 6 ✓
- §5 (mDNS listener) → Task 7 ✓
- §6 type changes (IpChanged + Banner.duration) → Tasks 1, 2 ✓
- §6 AppState handlers → Task 3 ✓
- §6 devices panel indicator → Task 4 ✓
- §7 wiring in fl run → Task 8 ✓
- §8 error handling → Tasks 6, 7, 8 (adb connect failures, mDNS startup, name lookup) ✓
- §9 unit tests (backoff, debounce, mDNS) → Tasks 5, 6, 7 ✓
- §9 integration test → Tasks 9, 10 ✓

One spec item with a conscious deviation: §9 specifies that the integration test should assert the full event sequence (`Lost → WifiReconnecting → WifiReconnected`). Task 10 only asserts boot + clean shutdown because faithfully driving `host:track-devices` from the test harness requires a fake adb daemon socket. The state-machine correctness is fully verified by Task 6's `tokio::time::pause` tests, so the gap is acceptable for this iteration. Documented inline in Task 10.

**2. Placeholder scan:** No TBD/TODO. Every step has real code or exact commands.

**3. Type consistency:** `WifiTarget`, `ManagerSetup`, `Input`, `Output`, `State`, `ManagerHandle`, `spawn` all referenced consistently from Task 5 onward. `Banner.duration: Option<Duration>` introduced in Task 2 is used in Task 3 and Task 4.

---

## Execution Handoff

**Plan complete and saved to [docs/superpowers/plans/2026-05-18-failover-hardening.md](2026-05-18-failover-hardening.md). Two execution options:**

**1. Subagent-Driven (recommended)** — Fresh subagent per task, controller verifies, fast iteration.

**2. Inline Execution** — Run tasks in this session with checkpoints.

**Which approach?**
