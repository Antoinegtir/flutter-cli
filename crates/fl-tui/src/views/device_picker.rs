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
        Self {
            devices: Vec::new(),
            cursor: 0,
            outcome: None,
            quitting: false,
        }
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
        self.devices
            .iter()
            .filter(|(_, c)| *c)
            .map(|(d, _)| d.serial.clone())
            .collect()
    }
}

impl View for DevicePickerView {
    type Input = DevicePickerInput;

    fn apply(&mut self, input: Self::Input) {
        match input {
            DevicePickerInput::DeviceFound(d) => {
                if !self
                    .devices
                    .iter()
                    .any(|(existing, _)| existing.serial == d.serial)
                {
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
            let plat_raw = d.platform.as_deref().unwrap_or("");
            let plat_label = if plat_raw == "ios-simulator" {
                "ios-sim"
            } else {
                plat_raw
            };
            let plat_glyph = crate::panels::devices::platform_icon(plat_raw);
            let plat_field = if plat_glyph.is_empty() {
                format!("{plat_label:<9}")
            } else {
                format!("{plat_glyph}  {plat_label:<7}")
            };
            lines.push(Line::styled(
                format!(
                    "{arrow}{bullet} {:<22} {plat_field} {} · {}",
                    d.name, conn, d.serial
                ),
                if i == self.cursor {
                    Style::default().fg(theme.accent).bg(theme.bg)
                } else {
                    theme.base()
                },
            ));
        }
        if self.devices.is_empty() {
            lines.push(Line::styled(
                "(awaiting devices…)".to_string(),
                theme.dimmed(),
            ));
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
    fn quitting(&self) -> bool {
        self.quitting
    }
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
            platform: None,
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
        assert_eq!(
            v.outcome,
            Some(DevicePickerOutcome::Picked(vec!["A".into()]))
        );
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
            for x in 0..buf.area.width {
                text.push_str(buf.get(x, y).symbol());
            }
            text.push('\n');
        }
        assert!(text.contains("ios"), "missing platform tag, got:\n{text}");
    }
}
