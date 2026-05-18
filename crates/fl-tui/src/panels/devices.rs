//! Devices panel: active + backup device with WiFi/USB icons.

use crate::app::AppState;
use crate::theme::Theme;
use fl_core::{ConnectionKind, Device, DeviceState};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

fn line_for(device: Option<&Device>, active: bool, theme: &Theme) -> Vec<Line<'static>> {
    let Some(d) = device else {
        return vec![Line::styled("(aucun)".to_string(), theme.dimmed())];
    };
    let bullet = if active { '●' } else { '○' };
    let icon = match d.connection {
        ConnectionKind::Wifi => "🔗 WiFi",
        ConnectionKind::Usb => "⚡ USB",
    };
    let state_str = match d.state {
        DeviceState::Online => "✓",
        DeviceState::Offline => "✗",
        DeviceState::Unauthorized => "?",
        DeviceState::Connecting => "…",
    };
    let style_main = if active { theme.base() } else { theme.dimmed() };

    let mut out = Vec::new();
    out.push(Line::from(vec![
        Span::styled(format!("{bullet} "), Style::default().fg(if active { theme.success } else { theme.dim }).bg(theme.bg)),
        Span::styled(d.name.clone(), style_main),
        Span::raw("  "),
        Span::styled(icon.to_string(), theme.dimmed()),
        Span::raw("  "),
        Span::styled(state_str.to_string(), style_main),
    ]));
    let ip = d.ip.clone().unwrap_or_else(|| d.serial.clone());
    out.push(Line::styled(format!("  {ip}"), theme.dimmed()));
    out
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use fl_core::{ConnectionKind, DeviceState};

    fn dev_wifi() -> Device {
        Device {
            serial: "1.2.3.4:5555".into(),
            name: "Pixel 8".into(),
            model: Some("Pixel 8".into()),
            connection: ConnectionKind::Wifi,
            state: DeviceState::Online,
            ip: Some("1.2.3.4".into()),
            android_version: Some("14".into()),
            battery: Some(80),
        }
    }

    #[test]
    fn active_line_uses_filled_bullet() {
        let t = Theme::TOKYO_NIGHT;
        let d = dev_wifi();
        let lines = line_for(Some(&d), true, &t);
        let s: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(s.starts_with("● "));
    }

    #[test]
    fn backup_line_uses_hollow_bullet() {
        let t = Theme::TOKYO_NIGHT;
        let d = dev_wifi();
        let lines = line_for(Some(&d), false, &t);
        let s: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(s.starts_with("○ "));
    }

    #[test]
    fn missing_device_shows_aucun() {
        let t = Theme::TOKYO_NIGHT;
        let lines = line_for(None, false, &t);
        assert_eq!(lines.len(), 1);
        let s: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(s, "(aucun)");
    }

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
}
