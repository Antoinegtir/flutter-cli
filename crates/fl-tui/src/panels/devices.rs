//! Devices panel: render one row per active session.

use crate::app::{prefix_color_index, AppState};
use crate::theme::Theme;
use fl_core::{ConnectionKind, DeviceSessionState, DeviceSessionSummary};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

/// Map a Flutter daemon `platform` string (e.g. `ios`, `android-arm64`,
/// `web-javascript`, `darwin`) to a plain emoji that renders without a
/// Nerd Font. Returns an empty string for unknown values so the row
/// layout stays stable.
pub fn platform_icon(platform: &str) -> &'static str {
    let p = platform.to_ascii_lowercase();
    if p.starts_with("ios")
        || p.starts_with("ipad")
        || p.starts_with("watch")
        || p.contains("darwin")
        || p.contains("macos")
    {
        "🍎"
    } else if p.starts_with("android") {
        "🤖"
    } else if p.starts_with("web")
        || p.contains("chrome")
        || p.contains("firefox")
        || p.contains("edge")
    {
        "🌐"
    } else if p.starts_with("windows") || p.contains("win") {
        "🪟"
    } else if p.starts_with("linux") {
        "🐧"
    } else if p.starts_with("fuchsia") {
        "🟣"
    } else {
        ""
    }
}

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
    let plat_raw = session.platform.as_deref().unwrap_or("");
    let plat_label: &str = if plat_raw == "ios-simulator" { "ios-sim" } else { plat_raw };
    let plat_glyph = platform_icon(plat_raw);
    // " {glyph} {label}" with the label left-padded so subsequent
    // columns (connection icon, state) stay aligned across rows. We
    // pad to 9 so widths match the previous layout exactly.
    let plat_text = if plat_glyph.is_empty() {
        format!("{plat_label:<9}")
    } else {
        format!("{plat_glyph}  {plat_label:<7}")
    };
    let row1 = Line::from(vec![
        Span::styled(format!("{bullet} "), Style::default().fg(bullet_color).bg(theme.bg)),
        Span::styled(format!("[{:<8}] ", session.short_name), Style::default().fg(prefix_color).bg(theme.bg)),
        Span::styled(session.display_name.clone(), theme.base()),
        Span::raw("  "),
        Span::styled(plat_text, theme.dimmed()),
        Span::raw(" "),
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

    #[test]
    fn render_includes_platform_tag() {
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(fl_core::AppEvent::Device(fl_core::DeviceEvent::SessionState {
            serial: "ABC".into(),
            state: fl_core::DeviceSessionState::Ready,
        }));
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
        assert!(
            text.contains("🍎"),
            "expected apple emoji next to ios platform, got:\n{text}"
        );
    }

    #[test]
    fn platform_icon_maps_known_strings() {
        assert_eq!(platform_icon("ios"), "🍎");
        assert_eq!(platform_icon("ios-simulator"), "🍎");
        assert_eq!(platform_icon("ipados"), "🍎");
        assert_eq!(platform_icon("watchos"), "🍎");
        assert_eq!(platform_icon("darwin-arm64"), "🍎");
        assert_eq!(platform_icon("android-arm64"), "🤖");
        assert_eq!(platform_icon("web-javascript"), "🌐");
        assert_eq!(platform_icon("windows-x64"), "🪟");
        assert_eq!(platform_icon("linux-x64"), "🐧");
        assert_eq!(platform_icon(""), "");
        assert_eq!(platform_icon("unknownos"), "");
    }
}
