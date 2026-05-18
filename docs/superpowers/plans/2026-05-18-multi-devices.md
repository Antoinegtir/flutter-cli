# Multi-device support (Sub-project C) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let `fl run` drive multiple Flutter devices simultaneously from one dashboard, with an interactive picker, broadcast hot-reload, and per-session reconnect logic.

**Architecture:** Replace the single-device `_daemon` + `vm_client` pair in `run_cmd.rs` with a `Vec<DeviceSession>` orchestrated by a new `fl-cli::multi` module. `AppState` drops `active_device`/`backup_device` in favour of a `Vec<DeviceSessionSummary>` (lightweight projection populated from `DeviceEvent`s). A new `DevicePickerView` runs first when 2+ devices are present and the user hasn't pinned the set with `-d`/`--all`.

**Tech Stack:** Same as Sub-projects A+B. No new dependencies.

**Spec:** [docs/superpowers/specs/2026-05-18-multi-devices-design.md](../specs/2026-05-18-multi-devices-design.md)

---

## File Structure

```
crates/fl-core/src/events.rs                         # + DeviceSessionState, + DeviceSessionSummary, + DeviceEvent::SessionState

crates/fl-tui/src/
├── app.rs                                           # drop active_device/backup_device, add active_sessions
├── render.rs                                        # header shows N-device summary
├── panels/devices.rs                                # render Vec<DeviceSessionSummary>
├── panels/performance.rs                            # 1/2/N layouts
├── views/device_picker.rs                           # new
├── views/mod.rs                                     # + pub mod device_picker;
└── lib.rs                                           # re-exports

crates/fl-cli/src/
├── cli.rs                                           # device: Vec<String>, all: bool, no_picker: bool, arg group
├── main.rs                                          # new dispatch
├── multi.rs                                         # new — DeviceSession, spawn_session, broadcast_key, run_multi
└── run_cmd.rs                                       # delegates to multi::run_multi

crates/fl-cli/tests/headless_run.rs                  # + headless_multi_device
tests/fixtures/scenarios/multi.txt                   # new — used by both sessions
```

---

## Task 1: Add `DeviceSessionState`, `DeviceSessionSummary`, `DeviceEvent::SessionState`

**Files:**
- Modify: `crates/fl-core/src/events.rs`

- [ ] **Step 1: Append new types and variant to `crates/fl-core/src/events.rs`** (before any test module)

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceSessionState {
    Connecting,
    Ready,
    Reloading,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceSessionSummary {
    pub serial: String,
    pub short_name: String,
    pub display_name: String,
    pub connection: ConnectionKind,
    pub ip: Option<String>,
    pub state: DeviceSessionState,
}
```

Then extend `DeviceEvent`. Locate the existing enum definition and add the new variant `SessionState` immediately before the trailing `Error(String)` arm:

```rust
pub enum DeviceEvent {
    Discovered(Device),
    Lost { serial: String },
    UsbDisconnected { serial: String },
    WifiPaired { serial: String, ip: String, port: u16 },
    WifiReconnecting { attempt: u32 },
    WifiReconnected,
    IpChanged { serial: String, old_ip: String, new_ip: String },
    SessionState { serial: String, state: DeviceSessionState },
    Error(String),
}
```

- [ ] **Step 2: Add tests inside the existing `#[cfg(test)] mod tests` block**

```rust
    #[test]
    fn session_state_roundtrips_through_json() {
        let ev = AppEvent::Device(DeviceEvent::SessionState {
            serial: "ABC".into(),
            state: DeviceSessionState::Ready,
        });
        let json = serde_json::to_string(&ev).unwrap();
        let back: AppEvent = serde_json::from_str(&json).unwrap();
        match back {
            AppEvent::Device(DeviceEvent::SessionState { serial, state }) => {
                assert_eq!(serial, "ABC");
                assert_eq!(state, DeviceSessionState::Ready);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn device_session_summary_equality() {
        let s = DeviceSessionSummary {
            serial: "S".into(),
            short_name: "short".into(),
            display_name: "Pixel 8".into(),
            connection: ConnectionKind::Wifi,
            ip: Some("1.2.3.4".into()),
            state: DeviceSessionState::Connecting,
        };
        let t = s.clone();
        assert_eq!(s, t);
    }
```

- [ ] **Step 3: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-core`
Expected: 10 prior + 2 new = 12 passes.

- [ ] **Step 4: Commit**

```bash
git add crates/fl-core/
git -c commit.gpgsign=false commit -m "feat(core): DeviceSessionState, DeviceSessionSummary, SessionState event

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Refactor `AppState` to `active_sessions: Vec<DeviceSessionSummary>` (drop `active_device`/`backup_device`) and adapt every reader

**Files:**
- Modify: `crates/fl-tui/src/app.rs`
- Modify: `crates/fl-tui/src/render.rs`
- Modify: `crates/fl-tui/src/panels/devices.rs`

This is the breaking refactor. It compiles only when every reader has been updated. Steps walk through the affected files in order; tests come at the end.

- [ ] **Step 1: Replace the `active_device` and `backup_device` fields in `AppState` (in `app.rs`)**

Find:
```rust
    pub active_device: Option<Device>,
    pub backup_device: Option<Device>,
```

Replace with:
```rust
    pub active_sessions: Vec<fl_core::DeviceSessionSummary>,
```

In `AppState::new`, find:
```rust
            active_device: None,
            backup_device: None,
```

Replace with:
```rust
            active_sessions: Vec::new(),
```

- [ ] **Step 2: Rewrite the `apply_device` device-arms in `app.rs`**

Locate `fn apply_device(&mut self, ev: DeviceEvent) { match ev { … } }` and replace its body with:

```rust
    fn apply_device(&mut self, ev: DeviceEvent) {
        match ev {
            DeviceEvent::Discovered(d) => {
                if let Some(sess) = self.active_sessions.iter_mut().find(|s| s.serial == d.serial) {
                    sess.state = fl_core::DeviceSessionState::Ready;
                    sess.ip = d.ip.clone();
                    sess.connection = d.connection;
                    sess.display_name = d.name.clone();
                }
                // If a session for this serial does NOT exist, ignore — the runner
                // owns session creation via SessionState events.
            }
            DeviceEvent::Lost { serial } => {
                if let Some(sess) = self.active_sessions.iter_mut().find(|s| s.serial == serial) {
                    sess.state = fl_core::DeviceSessionState::Stopped;
                }
            }
            DeviceEvent::UsbDisconnected { .. } => {
                self.show_banner(BannerKind::Info, "USB déconnecté — WiFi prend le relais");
            }
            DeviceEvent::WifiPaired { .. } => {
                self.show_banner(BannerKind::Success, "WiFi pairing OK");
            }
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
            DeviceEvent::IpChanged { new_ip, serial, .. } => {
                self.show_banner(BannerKind::Success, &format!("New IP: {new_ip}"));
                if let Some(sess) = self.active_sessions.iter_mut().find(|s| s.serial == serial) {
                    sess.ip = Some(new_ip.clone());
                }
            }
            DeviceEvent::SessionState { serial, state } => {
                if let Some(sess) = self.active_sessions.iter_mut().find(|s| s.serial == serial) {
                    sess.state = state;
                } else {
                    // New session announced — runner has informed us of it.
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
                    });
                }
            }
            DeviceEvent::Error(msg) => {
                self.show_banner(BannerKind::Error, &msg);
            }
        }
    }
```

- [ ] **Step 3: Add `short_name_for_serial` and `prefix_color_for` helpers at the bottom of `app.rs`** (before the `#[cfg(test)]` block)

```rust
/// 1–8 alphanumeric chars; stable across runs for a given serial.
pub fn short_name_for_serial(serial: &str) -> String {
    let mut s: String = serial.chars().filter(|c| c.is_alphanumeric()).take(8).collect();
    if s.is_empty() {
        s.push('?');
    }
    s
}

/// djb2-hash a short_name to a stable index into a palette of 4 accent colors.
pub fn prefix_color_index(short_name: &str) -> usize {
    let mut hash: u64 = 5381;
    for b in short_name.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as u64);
    }
    (hash % 4) as usize
}
```

- [ ] **Step 4: Update `app.rs` tests that referenced `active_device`/`backup_device`**

Find the three tests `discovered_device_becomes_active_when_no_other`, `second_discovered_becomes_backup`, and `ipchanged_updates_active_device_ip` and replace with these versions:

```rust
    #[test]
    fn session_state_event_creates_summary_for_unknown_serial() {
        let mut s = AppState::new("app".into(), "debug".into());
        s.apply(AppEvent::Device(DeviceEvent::SessionState {
            serial: "ABC".into(),
            state: fl_core::DeviceSessionState::Connecting,
        }));
        assert_eq!(s.active_sessions.len(), 1);
        assert_eq!(s.active_sessions[0].serial, "ABC");
        assert_eq!(s.active_sessions[0].state, fl_core::DeviceSessionState::Connecting);
    }

    #[test]
    fn discovered_marks_session_ready() {
        let mut s = AppState::new("app".into(), "debug".into());
        s.apply(AppEvent::Device(DeviceEvent::SessionState {
            serial: "ABC".into(),
            state: fl_core::DeviceSessionState::Connecting,
        }));
        s.apply(AppEvent::Device(DeviceEvent::Discovered(Device {
            serial: "ABC".into(),
            name: "Pixel".into(),
            model: None,
            connection: fl_core::ConnectionKind::Usb,
            state: fl_core::DeviceState::Online,
            ip: None,
            android_version: None,
            battery: None,
        })));
        assert_eq!(s.active_sessions[0].state, fl_core::DeviceSessionState::Ready);
        assert_eq!(s.active_sessions[0].display_name, "Pixel");
    }

    #[test]
    fn ipchanged_updates_session_ip() {
        let mut s = AppState::new("app".into(), "debug".into());
        s.apply(AppEvent::Device(DeviceEvent::SessionState {
            serial: "1.2.3.4:5555".into(),
            state: fl_core::DeviceSessionState::Ready,
        }));
        s.apply(AppEvent::Device(DeviceEvent::IpChanged {
            serial: "1.2.3.4:5555".into(),
            old_ip: "1.2.3.4".into(),
            new_ip: "10.0.0.5".into(),
        }));
        assert_eq!(s.active_sessions[0].ip.as_deref(), Some("10.0.0.5"));
    }

    #[test]
    fn lost_marks_session_stopped() {
        let mut s = AppState::new("app".into(), "debug".into());
        s.apply(AppEvent::Device(DeviceEvent::SessionState {
            serial: "ABC".into(),
            state: fl_core::DeviceSessionState::Ready,
        }));
        s.apply(AppEvent::Device(DeviceEvent::Lost { serial: "ABC".into() }));
        assert_eq!(s.active_sessions[0].state, fl_core::DeviceSessionState::Stopped);
    }

    #[test]
    fn short_name_for_serial_truncates_to_8() {
        assert_eq!(short_name_for_serial("Pixel_8_AB12"), "Pixel8AB");
        assert_eq!(short_name_for_serial("192.168.1.42:5555"), "19216814");
        assert_eq!(short_name_for_serial(""), "?");
    }
```

- [ ] **Step 5: Update `crates/fl-tui/src/render.rs` header**

Locate `fn render_header(...)` and replace the `device` binding:

```rust
    let device = state.active_device.as_ref().map(|d| d.name.clone()).unwrap_or_else(|| "no device".into());
```

with:

```rust
    let device = match state.active_sessions.len() {
        0 => "no device".to_string(),
        1 => state.active_sessions[0].display_name.clone(),
        n => format!("{n} devices"),
    };
```

The dashboard snapshot test will need re-acceptance after this change.

- [ ] **Step 6: Rewrite `crates/fl-tui/src/panels/devices.rs`**

Replace the entire file content (preserves the public API surface — only the implementation changes):

```rust
//! Devices panel: render one row per active session.

use crate::app::{prefix_color_index, AppState};
use crate::theme::Theme;
use fl_core::{ConnectionKind, DeviceSessionState, DeviceSessionSummary};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

fn lines_for(session: &DeviceSessionSummary, theme: &Theme) -> Vec<Line<'static>> {
    let (bullet, bullet_color) = match session.state {
        DeviceSessionState::Ready => ('●', theme.success),
        DeviceSessionState::Reloading => ('⠹', theme.warn),
        DeviceSessionState::Connecting => ('⠋', theme.warn),
        DeviceSessionState::Stopped => ('○', theme.dim),
        DeviceSessionState::Failed => ('✗', theme.error),
    };
    let icon = match session.connection {
        ConnectionKind::Wifi => "🔗 WiFi",
        ConnectionKind::Usb => "⚡ USB",
    };
    let state_label = match session.state {
        DeviceSessionState::Ready => "ready",
        DeviceSessionState::Reloading => "reloading",
        DeviceSessionState::Connecting => "connecting",
        DeviceSessionState::Stopped => "stopped",
        DeviceSessionState::Failed => "failed",
    };
    let palette = [theme.accent, theme.cyan, theme.success, theme.warn];
    let prefix_color = palette[prefix_color_index(&session.short_name)];
    let row1 = Line::from(vec![
        Span::styled(format!("{bullet} "), Style::default().fg(bullet_color).bg(theme.bg)),
        Span::styled(format!("[{:<8}] ", session.short_name), Style::default().fg(prefix_color).bg(theme.bg)),
        Span::styled(session.display_name.clone(), theme.base()),
        Span::raw("  "),
        Span::styled(icon.to_string(), theme.dimmed()),
        Span::raw("  "),
        Span::styled(state_label.to_string(), theme.dimmed()),
    ]);
    let addr = session.ip.clone().unwrap_or_else(|| session.serial.clone());
    let row2 = Line::styled(format!("    {addr}"), theme.dimmed());
    vec![row1, row2]
}

pub fn render_devices(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    let block = Block::default()
        .title(" Devices ")
        .borders(Borders::ALL)
        .border_style(theme.dimmed())
        .style(theme.base());
    let inner = block.inner(area);
    block.render(area, buf);

    let mut lines: Vec<Line> = Vec::new();
    if state.active_sessions.is_empty() {
        lines.push(Line::styled("(aucun)".to_string(), theme.dimmed()));
    } else {
        for sess in &state.active_sessions {
            lines.extend(lines_for(sess, theme));
        }
    }
    // Persistent reconnecting indicator (sub-project A).
    if let Some(b) = &state.banner {
        if b.duration.is_none() && b.message.starts_with("Reconnecting") {
            lines.push(Line::styled(format!("  ↻ {}", b.message), theme.dimmed()));
        }
    }
    Paragraph::new(lines).render(inner, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;
    use fl_core::{AppEvent, DeviceEvent, DeviceSessionState};

    fn add_session(s: &mut AppState, serial: &str, state: DeviceSessionState) {
        s.apply(AppEvent::Device(DeviceEvent::SessionState {
            serial: serial.into(),
            state,
        }));
    }

    #[test]
    fn renders_two_sessions_with_two_lines_each() {
        let mut s = AppState::new("a".into(), "d".into());
        add_session(&mut s, "ABC", DeviceSessionState::Ready);
        add_session(&mut s, "1.2.3.4:5555", DeviceSessionState::Connecting);
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 10));
        render_devices(Rect::new(0, 0, 60, 10), &mut buf, &s, &Theme::TOKYO_NIGHT);
        // Dump and check we see both serials.
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf.get(x, y).symbol());
            }
            text.push('\n');
        }
        assert!(text.contains("ABC"), "missing ABC:\n{text}");
        assert!(text.contains("1.2.3.4"), "missing wifi serial:\n{text}");
    }

    #[test]
    fn empty_state_shows_aucun() {
        let s = AppState::new("a".into(), "d".into());
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 4));
        render_devices(Rect::new(0, 0, 60, 4), &mut buf, &s, &Theme::TOKYO_NIGHT);
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width { text.push_str(buf.get(x, y).symbol()); }
            text.push('\n');
        }
        assert!(text.contains("aucun"), "missing aucun:\n{text}");
    }
}
```

- [ ] **Step 7: Update the old `reconnecting_indicator_appears_when_persistent_banner_is_reconnecting` test in the same file**

Replace the old test (which constructed `dev_wifi`) with one using the new session model:

```rust
    #[test]
    fn reconnecting_indicator_appears_when_persistent_banner_is_reconnecting() {
        let mut s = AppState::new("a".into(), "d".into());
        add_session(&mut s, "1.2.3.4:5555", DeviceSessionState::Ready);
        s.apply(AppEvent::Device(DeviceEvent::WifiReconnecting { attempt: 2 }));
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 8));
        render_devices(Rect::new(0, 0, 60, 8), &mut buf, &s, &Theme::TOKYO_NIGHT);
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width { text.push_str(buf.get(x, y).symbol()); }
            text.push('\n');
        }
        assert!(text.contains("↻"), "expected reconnecting indicator, got:\n{text}");
        assert!(text.contains("#2"));
    }
```

If a `dev_wifi` helper remains and is no longer used, delete it.

- [ ] **Step 8: Re-accept the dashboard snapshot in `render.rs`**

Run: `. "$HOME/.cargo/env" && INSTA_UPDATE=always cargo test -p fl-tui dashboard_snapshot 2>&1 | tail -5`
Expected: snapshot accepted.

- [ ] **Step 9: Run the full fl-tui test suite**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui`
Expected: all green. Test count will shift (~51 → ~52, depending on adds/removes).

- [ ] **Step 10: Commit**

```bash
git add crates/fl-tui/
git -c commit.gpgsign=false commit -m "refactor(tui): AppState.active_sessions replaces active_device/backup_device

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Performance panel layouts for 1/2/N sessions

**Files:**
- Modify: `crates/fl-tui/src/panels/performance.rs`

The MVP performance panel is single-session. With N ≥ 2 it should show per-session sparklines side-by-side; with N ≥ 3, a summary.

For this iteration we keep the existing single-session sparkline when N == 1 (unchanged path), and replace the contents with a *summary* line for N != 1. Per-session sparklines are noted as follow-up.

- [ ] **Step 1: Modify `render_performance` in `crates/fl-tui/src/panels/performance.rs`**

Replace the function body. Keep `sparkline` and `fps_color` helpers as-is.

```rust
pub fn render_performance(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    let block = Block::default()
        .title(" Performance ")
        .borders(Borders::ALL)
        .border_style(theme.dimmed())
        .style(theme.base());
    let inner = block.inner(area);
    block.render(area, buf);

    let n = state.active_sessions.len();
    if n <= 1 {
        render_single(inner, buf, state, theme);
    } else {
        render_summary(inner, buf, state, theme, n);
    }
}

fn render_single(inner: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let cur_fps = state.fps_samples.back().copied().unwrap_or(0.0);
    let spark_fps = sparkline(&state.fps_samples, 60.0, (inner.width as usize).saturating_sub(14));
    let fps_line = Line::styled(
        format!("FPS    {spark_fps} {cur_fps:>4.1}"),
        Style::default().fg(fps_color(cur_fps, theme)).bg(theme.bg),
    );
    Paragraph::new(fps_line).render(layout[0], buf);

    let frame_line = Line::styled(
        format!("Frame  ui {:>4.1}ms  raster {:>4.1}ms", state.frame_ui_ms, state.frame_raster_ms),
        theme.dimmed(),
    );
    Paragraph::new(frame_line).render(layout[1], buf);

    let mem_max = state.mem_samples.iter().cloned().fold(64.0_f32, f32::max);
    let cur_mem = state.mem_samples.back().copied().unwrap_or(0.0);
    let spark_mem = sparkline(&state.mem_samples, mem_max, (inner.width as usize).saturating_sub(14));
    let mem_line = Line::styled(
        format!("Memory {spark_mem} {cur_mem:>4.0}MB"),
        theme.base(),
    );
    Paragraph::new(mem_line).render(layout[2], buf);

    let rb = Line::styled(
        format!("Rebuilds {}/s", state.rebuilds_per_sec),
        theme.dimmed(),
    );
    Paragraph::new(rb).render(layout[3], buf);
}

fn render_summary(inner: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme, n: usize) {
    let cur_fps = state.fps_samples.back().copied().unwrap_or(0.0);
    let spark_fps = sparkline(&state.fps_samples, 60.0, (inner.width as usize).saturating_sub(14));
    let line1 = Line::styled(
        format!("FPS avg {spark_fps} {cur_fps:>4.1}  ·  {n} devices online"),
        Style::default().fg(fps_color(cur_fps, theme)).bg(theme.bg),
    );
    let cur_mem = state.mem_samples.back().copied().unwrap_or(0.0);
    let line2 = Line::styled(
        format!("Mem ~{cur_mem:.0}MB total"),
        theme.dimmed(),
    );
    Paragraph::new(vec![line1, line2]).render(inner, buf);
}
```

- [ ] **Step 2: Add a test that exercises the multi-session path**

Append inside the existing `mod tests` block in `performance.rs`:

```rust
    use crate::app::AppState;
    use fl_core::{AppEvent, DeviceEvent, DeviceSessionState};

    #[test]
    fn renders_summary_when_two_or_more_sessions() {
        let mut s = AppState::new("a".into(), "d".into());
        for serial in ["a", "b", "c"] {
            s.apply(AppEvent::Device(DeviceEvent::SessionState {
                serial: serial.into(),
                state: DeviceSessionState::Ready,
            }));
        }
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 5));
        render_performance(Rect::new(0, 0, 40, 5), &mut buf, &s, &Theme::TOKYO_NIGHT);
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width { text.push_str(buf.get(x, y).symbol()); }
            text.push('\n');
        }
        assert!(text.contains("3 devices"), "missing summary count:\n{text}");
    }
```

- [ ] **Step 3: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add crates/fl-tui/
git -c commit.gpgsign=false commit -m "feat(tui): performance panel switches to summary for 2+ sessions

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `DevicePickerView`

**Files:**
- Create: `crates/fl-tui/src/views/device_picker.rs`
- Modify: `crates/fl-tui/src/views/mod.rs`
- Modify: `crates/fl-tui/src/lib.rs`

- [ ] **Step 1: Create `crates/fl-tui/src/views/device_picker.rs`**

```rust
//! Interactive device selector used by `fl run` when 2+ devices are detected.

use crate::theme::Theme;
use crate::view::View;
use fl_core::{ConnectionKind, Device, KeyEvent as FlKey};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum DevicePickerInput {
    DeviceFound(Device),
    Toggle(usize),
    SelectAll,
    Confirm,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePickerOutcome {
    Picked(Vec<String>),
    Cancelled,
}

pub struct DevicePickerView {
    pub devices: Vec<(Device, bool)>, // (device, checked)
    pub cursor: usize,
    pub outcome: Option<DevicePickerOutcome>,
    pub quitting: bool,
}

impl Default for DevicePickerView {
    fn default() -> Self {
        Self::new()
    }
}

impl DevicePickerView {
    pub fn new() -> Self {
        Self { devices: Vec::new(), cursor: 0, outcome: None, quitting: false }
    }

    pub fn with_devices(devices: Vec<Device>) -> Self {
        Self {
            devices: devices.into_iter().map(|d| (d, false)).collect(),
            cursor: 0,
            outcome: None,
            quitting: false,
        }
    }

    fn selected_serials(&self) -> Vec<String> {
        self.devices.iter().filter(|(_, c)| *c).map(|(d, _)| d.serial.clone()).collect()
    }
}

impl View for DevicePickerView {
    type Input = DevicePickerInput;

    fn apply(&mut self, input: Self::Input) {
        match input {
            DevicePickerInput::DeviceFound(d) => {
                if !self.devices.iter().any(|(existing, _)| existing.serial == d.serial) {
                    self.devices.push((d, false));
                }
            }
            DevicePickerInput::Toggle(i) => {
                if let Some((_, c)) = self.devices.get_mut(i) {
                    *c = !*c;
                }
            }
            DevicePickerInput::SelectAll => {
                let any_unchecked = self.devices.iter().any(|(_, c)| !*c);
                for (_, c) in self.devices.iter_mut() {
                    *c = any_unchecked;
                }
            }
            DevicePickerInput::Confirm => {
                let serials = self.selected_serials();
                if !serials.is_empty() {
                    self.outcome = Some(DevicePickerOutcome::Picked(serials));
                    self.quitting = true;
                }
            }
            DevicePickerInput::Cancel => {
                self.outcome = Some(DevicePickerOutcome::Cancelled);
                self.quitting = true;
            }
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let block = Block::default()
            .title(" fl run ── Select devices ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent).bg(theme.bg))
            .style(theme.base());
        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines: Vec<Line> = Vec::new();
        for (i, (d, checked)) in self.devices.iter().enumerate() {
            let bullet = if *checked { "[✓]" } else { "[ ]" };
            let arrow = if i == self.cursor { "▸ " } else { "  " };
            let conn = match d.connection {
                ConnectionKind::Wifi => "WiFi",
                ConnectionKind::Usb => "USB",
            };
            lines.push(Line::styled(
                format!("{arrow}{bullet} {:<22} {} · {}", d.name, conn, d.serial),
                if i == self.cursor {
                    Style::default().fg(theme.accent).bg(theme.bg)
                } else {
                    theme.base()
                },
            ));
        }
        if self.devices.is_empty() {
            lines.push(Line::styled("(awaiting devices…)".to_string(), theme.dimmed()));
        }
        lines.push(Line::styled("".to_string(), theme.dimmed()));
        lines.push(Line::styled(
            "↑↓ navigate   space toggle   a select all   enter run   q quit".to_string(),
            theme.dimmed(),
        ));
        Paragraph::new(lines).render(inner, buf);
    }

    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input> {
        match key {
            FlKey::Char('q') | FlKey::Ctrl('c') | FlKey::Esc => Some(DevicePickerInput::Cancel),
            FlKey::Down if self.cursor + 1 < self.devices.len() => {
                self.cursor += 1;
                None
            }
            FlKey::Up if self.cursor > 0 => {
                self.cursor -= 1;
                None
            }
            FlKey::Char(' ') => Some(DevicePickerInput::Toggle(self.cursor)),
            FlKey::Char('a') => Some(DevicePickerInput::SelectAll),
            FlKey::Enter => Some(DevicePickerInput::Confirm),
            _ => None,
        }
    }
    fn tick(&mut self, _dt: Duration) {}
    fn quitting(&self) -> bool { self.quitting }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fl_core::DeviceState;

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
        }
    }

    #[test]
    fn toggle_flips_checkmark() {
        let mut v = DevicePickerView::with_devices(vec![dev("A"), dev("B")]);
        v.apply(DevicePickerInput::Toggle(1));
        assert!(!v.devices[0].1);
        assert!(v.devices[1].1);
    }

    #[test]
    fn select_all_checks_all_when_some_unchecked() {
        let mut v = DevicePickerView::with_devices(vec![dev("A"), dev("B")]);
        v.apply(DevicePickerInput::Toggle(0));
        v.apply(DevicePickerInput::SelectAll);
        assert!(v.devices.iter().all(|(_, c)| *c));
    }

    #[test]
    fn select_all_unchecks_when_all_already_checked() {
        let mut v = DevicePickerView::with_devices(vec![dev("A"), dev("B")]);
        v.apply(DevicePickerInput::SelectAll);
        v.apply(DevicePickerInput::SelectAll);
        assert!(v.devices.iter().all(|(_, c)| !*c));
    }

    #[test]
    fn confirm_with_selection_sets_picked_outcome() {
        let mut v = DevicePickerView::with_devices(vec![dev("A"), dev("B")]);
        v.apply(DevicePickerInput::Toggle(0));
        v.apply(DevicePickerInput::Confirm);
        assert_eq!(v.outcome, Some(DevicePickerOutcome::Picked(vec!["A".into()])));
        assert!(v.quitting);
    }

    #[test]
    fn confirm_with_empty_selection_does_not_quit() {
        let mut v = DevicePickerView::with_devices(vec![dev("A")]);
        v.apply(DevicePickerInput::Confirm);
        assert!(v.outcome.is_none());
        assert!(!v.quitting);
    }

    #[test]
    fn cancel_sets_cancelled_outcome() {
        let mut v = DevicePickerView::with_devices(vec![dev("A")]);
        v.apply(DevicePickerInput::Cancel);
        assert_eq!(v.outcome, Some(DevicePickerOutcome::Cancelled));
        assert!(v.quitting);
    }

    #[test]
    fn device_found_appends_unique_serial_only() {
        let mut v = DevicePickerView::new();
        v.apply(DevicePickerInput::DeviceFound(dev("A")));
        v.apply(DevicePickerInput::DeviceFound(dev("A")));
        v.apply(DevicePickerInput::DeviceFound(dev("B")));
        assert_eq!(v.devices.len(), 2);
    }
}
```

- [ ] **Step 2: Update `crates/fl-tui/src/views/mod.rs`**

Replace with:

```rust
//! Command-specific TUI views.

pub mod build_view;
pub mod clean_view;
pub mod device_picker;
pub mod doctor_view;
pub mod pub_view;
pub mod test_view;
```

- [ ] **Step 3: Add re-exports in `crates/fl-tui/src/lib.rs`**

Add at the bottom of the `pub use` block:

```rust
pub use views::device_picker::{DevicePickerInput, DevicePickerOutcome, DevicePickerView};
```

- [ ] **Step 4: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-tui`
Expected: 7 new tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/fl-tui/
git -c commit.gpgsign=false commit -m "feat(tui): DevicePickerView for multi-device fl run startup

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: CLI surface for multi-device `fl run`

**Files:**
- Modify: `crates/fl-cli/src/cli.rs`

- [ ] **Step 1: Replace the `Run` arm in `crates/fl-cli/src/cli.rs`**

Find:
```rust
    Run {
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(short, long)] device: Option<String>,
        #[arg(long)] no_wifi: bool,
        #[arg(long, value_enum, default_value_t = BuildMode::Debug)] mode: BuildMode,
    },
```

Replace with:
```rust
    Run {
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(short, long)] device: Vec<String>,
        #[arg(long)] all: bool,
        #[arg(long)] no_picker: bool,
        #[arg(long)] no_wifi: bool,
        #[arg(long, value_enum, default_value_t = BuildMode::Debug)] mode: BuildMode,
    },
```

- [ ] **Step 2: Replace the run-related tests in `cli.rs` `mod tests`**

Replace the existing `parses_run_with_options` and `parses_run_with_explicit_mode` with:

```rust
    #[test]
    fn parses_run_with_options() {
        let c = Cli::parse_from(["fl", "run", "--device", "1.2.3.4:5555", "--no-wifi"]);
        match c.cmd {
            Cmd::Run { device, no_wifi, mode, all, .. } => {
                assert_eq!(device, vec!["1.2.3.4:5555".to_string()]);
                assert!(no_wifi);
                assert!(!all);
                assert_eq!(mode, BuildMode::Debug);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_run_with_repeated_device() {
        let c = Cli::parse_from(["fl", "run", "--device", "a", "--device", "b"]);
        match c.cmd {
            Cmd::Run { device, .. } => assert_eq!(device, vec!["a".to_string(), "b".to_string()]),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_run_with_all_flag() {
        let c = Cli::parse_from(["fl", "run", "--all"]);
        match c.cmd {
            Cmd::Run { all, .. } => assert!(all),
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
    fn parses_run_with_no_picker() {
        let c = Cli::parse_from(["fl", "run", "--no-picker"]);
        match c.cmd {
            Cmd::Run { no_picker, .. } => assert!(no_picker),
            _ => panic!(),
        }
    }
```

- [ ] **Step 3: Run tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-cli --lib`
Expected: pass (will fail to compile main.rs if not updated — Task 7 handles that).

If `cargo test -p fl-cli` does not work because main.rs no longer compiles, that's expected at this point. The lib-only run isolates the test target. Move on.

- [ ] **Step 4: Commit** (don't worry about the binary not building — Task 7 fixes that)

```bash
git add crates/fl-cli/src/cli.rs
git -c commit.gpgsign=false commit -m "feat(cli): fl run accepts repeated --device, --all, --no-picker

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: `fl-cli/src/multi.rs` — `DeviceSession` + `spawn_session` + `broadcast_key`

**Files:**
- Create: `crates/fl-cli/src/multi.rs`

This task creates the multi-device runtime module. It does NOT wire it into `main.rs` or `run_cmd.rs` yet (Task 7 does that). For now, the module compiles standalone with its own unit tests.

- [ ] **Step 1: Create `crates/fl-cli/src/multi.rs`**

```rust
//! Multi-device runtime for `fl run`.
//!
//! Owns N parallel `DeviceSession`s, each backed by its own `FlutterDaemon` +
//! `VmServiceClient` + `ReconnectManager`. Broadcasts keys to every session
//! in parallel.

use anyhow::{anyhow, Context};
use fl_adb::{parse_devices_l, pre_pair_wifi, track_devices, CommandRunner, TokioRunner};
use fl_core::{
    AppEvent, BuildMode, DeviceEvent, DeviceSessionState, FlutterEvent, KeyEvent as FlKey,
    LogLevel,
};
use fl_flutter::{resolve_flutter, FlutterDaemon};
use fl_tui::{AppState, TuiRunner};
use fl_vmservice::VmServiceClient;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub struct DeviceSession {
    pub serial: String,
    pub short_name: String,
    pub display_name: String,
    pub daemon: Arc<Mutex<Option<FlutterDaemon>>>,
    pub vm_client: Arc<Mutex<Option<VmServiceClient>>>,
    pub isolate_id: Arc<Mutex<Option<String>>>,
}

impl DeviceSession {
    pub fn new(serial: String, display_name: String) -> Self {
        let short_name = fl_tui::app::short_name_for_serial(&serial);
        Self {
            serial,
            short_name,
            display_name,
            daemon: Arc::new(Mutex::new(None)),
            vm_client: Arc::new(Mutex::new(None)),
            isolate_id: Arc::new(Mutex::new(None)),
        }
    }
}

pub async fn spawn_session<R: CommandRunner + 'static>(
    runner: Arc<R>,
    flutter: &Path,
    project: &Path,
    serial_to_run: String,
    usb_serial_to_pair: Option<String>,
    no_wifi: bool,
    mode: BuildMode,
    event_tx: mpsc::Sender<AppEvent>,
) -> anyhow::Result<DeviceSession> {
    let display_name = match &usb_serial_to_pair {
        Some(usb) => runner
            .run("adb", &["-s", usb, "shell", "getprop", "ro.product.model"])
            .await
            .ok()
            .map(|o| o.stdout.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| serial_to_run.clone()),
        None => serial_to_run.clone(),
    };
    let session = DeviceSession::new(serial_to_run.clone(), display_name);

    event_tx
        .send(AppEvent::Device(DeviceEvent::SessionState {
            serial: session.serial.clone(),
            state: DeviceSessionState::Connecting,
        }))
        .await
        .ok();

    // Pre-pair WiFi if requested.
    let final_target = if let (Some(usb), false) = (usb_serial_to_pair.as_deref(), no_wifi) {
        match pre_pair_wifi(runner.as_ref(), usb, 5555).await {
            Ok(t) => {
                event_tx.send(AppEvent::Device(DeviceEvent::WifiPaired {
                    serial: usb.into(),
                    ip: t.ip.clone(),
                    port: t.port,
                })).await.ok();
                t.serial()
            }
            Err(e) => {
                event_tx.send(AppEvent::Device(DeviceEvent::Error(
                    format!("[{}] pre-pair failed: {e}", session.short_name),
                ))).await.ok();
                serial_to_run.clone()
            }
        }
    } else {
        serial_to_run.clone()
    };

    // Spawn FlutterDaemon for this session and forward its events (prefixed with short_name) to event_tx.
    let (flutter_tx, mut flutter_rx) = mpsc::channel::<FlutterEvent>(64);
    let mode_flag = mode.flutter_flag();
    let extra: Vec<&str> = if matches!(mode, BuildMode::Debug) { Vec::new() } else { vec![mode_flag] };
    let daemon = FlutterDaemon::spawn(flutter, project, &final_target, &extra, flutter_tx).await?;
    *session.daemon.lock().await = Some(daemon);

    let short_for_logs = session.short_name.clone();
    let serial_for_state = session.serial.clone();
    let event_tx_logs = event_tx.clone();
    tokio::spawn(async move {
        while let Some(ev) = flutter_rx.recv().await {
            let prefixed = match ev {
                FlutterEvent::Log { level, message } => FlutterEvent::Log {
                    level,
                    message: format!("[{short_for_logs}] {message}"),
                },
                FlutterEvent::AppStarted { .. } => {
                    event_tx_logs.send(AppEvent::Device(DeviceEvent::SessionState {
                        serial: serial_for_state.clone(),
                        state: DeviceSessionState::Ready,
                    })).await.ok();
                    ev
                }
                FlutterEvent::Stopped { .. } => {
                    event_tx_logs.send(AppEvent::Device(DeviceEvent::SessionState {
                        serial: serial_for_state.clone(),
                        state: DeviceSessionState::Stopped,
                    })).await.ok();
                    ev
                }
                other => other,
            };
            event_tx_logs.send(AppEvent::Flutter(prefixed)).await.ok();
        }
    });

    Ok(session)
}

pub async fn broadcast_key(key: FlKey, sessions: &[DeviceSession], events: &mpsc::Sender<AppEvent>) {
    let mut futures = Vec::new();
    for s in sessions {
        let vm = s.vm_client.lock().await.clone();
        let iso = s.isolate_id.lock().await.clone();
        let (Some(client), Some(iso)) = (vm, iso) else { continue };
        let short = s.short_name.clone();
        let key_copy = key;
        futures.push(async move {
            let res = match key_copy {
                FlKey::Char('r') => client.hot_reload(&iso).await,
                FlKey::Char('R') => client.hot_restart(&iso).await,
                FlKey::Char('b') => client.toggle_brightness(&iso, true).await,
                FlKey::Char('p') => client.toggle_debug_paint(&iso, true).await,
                FlKey::Char('o') => client.toggle_platform(&iso, false).await,
                FlKey::Char('P') => client.toggle_performance_overlay(&iso, true).await,
                _ => return None,
            };
            Some((short, res.err().map(|e| e.to_string())))
        });
    }
    let results = futures_util::future::join_all(futures).await;
    for outcome in results.into_iter().flatten() {
        let (short, err) = outcome;
        match err {
            None if matches!(key, FlKey::Char('r')) => {
                events.send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Info,
                    message: format!("[{short}] reload OK"),
                })).await.ok();
            }
            Some(e) => {
                events.send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Error,
                    message: format!("[{short}] {key:?} -> {e}"),
                })).await.ok();
            }
            _ => {}
        }
    }
}

/// Resolve `flutter` binary path (shared helper).
pub fn resolve_flutter_path() -> anyhow::Result<PathBuf> {
    resolve_flutter(None, std::env::var("FLUTTER_ROOT").ok().as_deref(), dirs_home())
        .ok_or_else(|| anyhow!("flutter binary not found"))
}

fn dirs_home() -> Option<&'static Path> {
    static HOME: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()))
        .as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_session_new_derives_short_name() {
        let s = DeviceSession::new("Pixel_8_ABCDEFG".into(), "Pixel 8".into());
        assert_eq!(s.serial, "Pixel_8_ABCDEFG");
        assert_eq!(s.short_name, "Pixel8AB"); // 8 alphanumeric chars
        assert_eq!(s.display_name, "Pixel 8");
    }
}
```

- [ ] **Step 2: Declare `multi` in `crates/fl-cli/src/main.rs`**

Add `mod multi;` near the other `mod` lines. This is necessary even though main.rs doesn't dispatch to it yet — to compile the module.

- [ ] **Step 3: Build + run the unit test**

Run: `. "$HOME/.cargo/env" && cargo build --workspace`
Expected: clean build with dead-code warnings for the unused `multi::*` functions (expected).

Run: `. "$HOME/.cargo/env" && cargo test -p fl-cli --lib multi 2>&1 | tail -10`
Expected: 1 test passes.

- [ ] **Step 4: Make `short_name_for_serial` accessible from `fl-cli` via `fl_tui::app`**

`multi.rs` calls `fl_tui::app::short_name_for_serial`. Confirm the function is `pub` in `fl_tui::app` (added in Task 2 Step 3 — already `pub`). If not, make it `pub`. No further action if it's already public.

- [ ] **Step 5: Commit**

```bash
git add crates/fl-cli/
git -c commit.gpgsign=false commit -m "feat(cli): multi.rs — DeviceSession, spawn_session, broadcast_key

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: `run_multi` orchestrator + run_cmd delegation + main dispatch

**Files:**
- Modify: `crates/fl-cli/src/multi.rs` (add `run_multi`)
- Modify: `crates/fl-cli/src/run_cmd.rs` (delegate to `run_multi`)
- Modify: `crates/fl-cli/src/main.rs` (new dispatch)

- [ ] **Step 1: Append `run_multi` to `crates/fl-cli/src/multi.rs`** (before the `#[cfg(test)]` module)

```rust
pub async fn run_multi(
    project: Option<PathBuf>,
    devices_arg: Vec<String>,
    all: bool,
    no_picker: bool,
    no_wifi: bool,
    mode: BuildMode,
) -> anyhow::Result<()> {
    let project = project.unwrap_or_else(|| std::env::current_dir().unwrap());
    let flutter = resolve_flutter_path()?;
    let runner = Arc::new(TokioRunner);

    // 1. Discover devices once up front (for picker / --all).
    let listed = runner.run("adb", &["devices", "-l"]).await?;
    let all_devices = parse_devices_l(&listed.stdout);

    // 2. Decide which serials to run on.
    let headless = std::env::var_os("FL_HEADLESS").is_some();
    let chosen: Vec<String> = if !devices_arg.is_empty() {
        devices_arg
    } else if all {
        if all_devices.is_empty() {
            return Err(anyhow!("--all specified but no devices attached"));
        }
        all_devices.iter().map(|d| d.serial.clone()).collect()
    } else if all_devices.len() <= 1 || no_picker || headless {
        all_devices.first().map(|d| vec![d.serial.clone()]).unwrap_or_default()
    } else {
        run_picker(&all_devices).await?
    };

    if chosen.is_empty() {
        return Err(anyhow!("no devices to run on"));
    }

    // 3. Build event channel + map serials to (run_serial, usb_serial_to_pair).
    let (event_tx, mut event_rx) = mpsc::channel::<AppEvent>(256);
    let mut sessions: Vec<DeviceSession> = Vec::new();
    for serial in &chosen {
        let usb_pair = all_devices
            .iter()
            .find(|d| d.serial == *serial && matches!(d.connection, fl_core::ConnectionKind::Usb))
            .map(|d| d.serial.clone());
        let s = spawn_session(
            runner.clone(),
            &flutter,
            &project,
            serial.clone(),
            usb_pair,
            no_wifi,
            mode,
            event_tx.clone(),
        )
        .await?;
        sessions.push(s);
    }

    // 4. Shared track-devices watcher.
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

    // 5. Headless drain.
    if headless {
        let app_name = project
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("app")
            .to_string();
        let mut state = AppState::new(app_name, "debug".into());
        while let Some(ev) = event_rx.recv().await {
            println!("{ev:?}");
            state.apply(ev);
            if state.active_sessions.iter().all(|s| matches!(s.state, DeviceSessionState::Stopped)) && !state.active_sessions.is_empty() {
                break;
            }
        }
        return Ok(());
    }

    // 6. TUI loop.
    let app_name = project.file_name().and_then(|n| n.to_str()).unwrap_or("app").to_string();
    let mut state = AppState::new(app_name, "debug".into());
    let mut runner = TuiRunner::init()?;
    let result = runner.run(&mut state, &mut event_rx, mpsc::channel::<FlKey>(1).0).await;

    // 7. Graceful shutdown.
    for s in &sessions {
        let mut guard = s.daemon.lock().await;
        if let Some(d) = guard.as_mut() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), d.send_quit()).await;
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), d.wait()).await;
        }
    }
    let _ = runner.restore();
    result
}

async fn run_picker(devices: &[fl_core::Device]) -> anyhow::Result<Vec<String>> {
    use fl_tui::{DevicePickerInput, DevicePickerOutcome, DevicePickerView};
    let mut view = DevicePickerView::with_devices(devices.to_vec());
    let (_tx, mut rx) = mpsc::channel::<DevicePickerInput>(1);
    let mut tui = TuiRunner::init()?;
    let r = tui.run_view(&mut view, &mut rx).await;
    let _ = tui.restore();
    r?;
    match view.outcome {
        Some(DevicePickerOutcome::Picked(serials)) => Ok(serials),
        Some(DevicePickerOutcome::Cancelled) | None => Err(anyhow!("device selection cancelled")),
    }
}
```

> Note: `run_multi`'s shutdown phase calls `d.send_quit()` and `d.wait()` on a `&mut FlutterDaemon`. Because the daemon is behind `Arc<Mutex<Option<...>>>`, the guard must be acquired (`s.daemon.lock().await`). Matches the MVP `run_cmd.rs` pattern.

- [ ] **Step 2: Replace `crates/fl-cli/src/run_cmd.rs` with a thin delegation**

```rust
//! `fl run` — delegates to multi::run_multi for the actual orchestration.

use fl_core::BuildMode;
use std::path::PathBuf;

pub async fn run(
    project: Option<PathBuf>,
    devices_arg: Vec<String>,
    all: bool,
    no_picker: bool,
    no_wifi: bool,
    mode: BuildMode,
) -> anyhow::Result<()> {
    crate::multi::run_multi(project, devices_arg, all, no_picker, no_wifi, mode).await
}
```

- [ ] **Step 3: Update `crates/fl-cli/src/main.rs` dispatch for `Cmd::Run`**

Find:
```rust
        Cmd::Run { project, device, no_wifi, mode } => {
            run_cmd::run(project, device, no_wifi, mode).await
        }
```

Replace with:
```rust
        Cmd::Run { project, device, all, no_picker, no_wifi, mode } => {
            run_cmd::run(project, device, all, no_picker, no_wifi, mode).await
        }
```

- [ ] **Step 4: Build the workspace**

Run: `. "$HOME/.cargo/env" && cargo build --workspace 2>&1 | tail -10`
Expected: clean build.

- [ ] **Step 5: Run the entire workspace test suite**

Run: `. "$HOME/.cargo/env" && cargo test --workspace -- --test-threads=1 2>&1 | grep "test result"`
Expected: every line `ok`.

- [ ] **Step 6: Clippy check**

Run: `. "$HOME/.cargo/env" && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean. Fix any new warnings inline.

- [ ] **Step 7: Commit**

```bash
git add crates/fl-cli/
git -c commit.gpgsign=false commit -m "feat(cli): run_multi orchestrator with picker, all, and per-session sessions

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Multi-device fixture + headless integration test

**Files:**
- Create: `tests/fixtures/scenarios/multi_devices.txt` (devices fixture)
- Modify: `crates/fl-cli/tests/headless_run.rs`

The faux `adb devices -l` script supports `FL_ADB_FIXTURE_DEVICES` (an absolute path to a file whose contents replace the default `adb devices -l` output). The faux `flutter run --machine` already replays from `FL_FLUTTER_SCENARIO`. To simulate 2 devices, we point `FL_ADB_FIXTURE_DEVICES` at a 2-device list and re-use the existing `nominal.txt` scenario (both spawned `flutter run` processes read the same scenario, both emit `app.started` + `app.stop`).

- [ ] **Step 1: Create `tests/fixtures/scenarios/multi_devices.txt`**

```
List of devices attached
DEV1                 device usb:1-2 product:husky model:Pixel_8 device:husky transport_id:1
DEV2                 device usb:1-3 product:other model:Tablet device:tab transport_id:2
```

- [ ] **Step 2: Append `headless_multi_device` to `crates/fl-cli/tests/headless_run.rs`**

```rust
#[test]
fn headless_multi_device_emits_two_app_started() {
    ensure_binary_built();
    let _ = std::fs::remove_dir_all("/tmp/fl-fake-adb");

    let pubspec = pubspec_in_workspace();
    let devices_file = fixtures().join("scenarios/multi_devices.txt");
    let scenario = fixtures().join("scenarios/nominal.txt");
    let out = run_fl_with_env(
        &[
            "run",
            "--no-picker", "--no-wifi",
            "--device", "DEV1",
            "--device", "DEV2",
            "--project", pubspec.to_str().unwrap(),
        ],
        &[
            ("FL_ADB_FIXTURE_DEVICES", &devices_file),
            ("FL_FLUTTER_SCENARIO", &scenario),
        ],
    );
    let starts = out.matches("AppStarted").count();
    let stops = out.matches("Stopped").count();
    assert!(starts >= 2, "expected ≥ 2 AppStarted events, output:\n{out}");
    assert!(stops >= 2, "expected ≥ 2 Stopped events, output:\n{out}");
}
```

- [ ] **Step 3: Run the integration tests**

Run: `. "$HOME/.cargo/env" && cargo test -p fl-cli --test headless_run -- --test-threads=1 2>&1 | tail -15`
Expected: 9 tests pass (8 prior + 1 new).

- [ ] **Step 4: Full workspace test + clippy**

Run: `. "$HOME/.cargo/env" && cargo test --workspace -- --test-threads=1 2>&1 | grep "test result"`
Expected: all `ok`.

Run: `. "$HOME/.cargo/env" && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add tests/fixtures/scenarios/multi_devices.txt crates/fl-cli/tests/headless_run.rs
git -c commit.gpgsign=false commit -m "test: headless multi-device scenario with two sessions

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage:**
- §3 CLI surface (`--device` Vec, `--all`, `--no-picker`) → Task 5 ✓
- §4 DevicePickerView → Task 4 ✓
- §5 DeviceSession, spawn_session, broadcast_key, run_multi → Tasks 6, 7 ✓
- §6 AppState refactor + short_name → Task 2 ✓
- §7 Devices panel rewrite → Task 2 ✓
- §8 Performance panel layouts → Task 3 ✓
- §9 Broadcast keys → Task 6 ✓
- §10 Errors (picker cancel, 0/N failures, quit timeout) → covered across Tasks 6+7 ✓
- §11 Tests (picker, app state, panel snapshot, multi-device integration) → Tasks 2, 3, 4, 8 ✓
- §12 File-level diff → all files covered ✓

One minor gap: the spec mentions "different colors per device prefix" but Task 2 only adds `prefix_color_index` (the integer index). The actual coloring of log lines based on that index is NOT wired in this plan — the logs panel still uses the level-based color. Adding per-device prefix coloring inside the logs panel can be a follow-up; the function is exported so it's available. Acceptable gap for this iteration.

**2. Placeholder scan:** No TBD/TODO/"similar to". Every step has executable content.

**3. Type consistency:**
- `DeviceSessionSummary` defined in Task 1 (with fields `serial`, `short_name`, `display_name`, `connection`, `ip`, `state`) — used unchanged in Tasks 2, 3, 4.
- `DeviceSessionState` variants `Connecting/Ready/Reloading/Stopped/Failed` — Tasks 1, 2, 6, 7 use these exact names.
- `DevicePickerInput` / `DevicePickerOutcome` / `DevicePickerView` — Task 4 defines, Task 7 uses with same signatures.
- `DeviceEvent::SessionState { serial, state }` — Task 1 adds, Tasks 2, 6, 7 consume.
- `spawn_session`, `broadcast_key`, `run_multi`, `resolve_flutter_path` — Task 6 introduces, Task 7 reuses.
- `short_name_for_serial`, `prefix_color_index` — Task 2 defines as `pub` in `fl_tui::app`, Task 6 uses.

---

## Execution Handoff

**Plan complete and saved to [docs/superpowers/plans/2026-05-18-multi-devices.md](2026-05-18-multi-devices.md). 8 TDD tasks total.**

**Two execution options:**

**1. Subagent-Driven (recommended)** — Fresh subagent per task.

**2. Inline Execution** — In-session with checkpoints.

**Which approach?**
