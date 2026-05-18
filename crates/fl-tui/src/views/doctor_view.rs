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

impl Default for DoctorView {
    fn default() -> Self {
        Self::new()
    }
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
            FlKey::Down if self.cursor + 1 < self.sections.len() => {
                self.cursor += 1;
            }
            FlKey::Up if self.cursor > 0 => {
                self.cursor -= 1;
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
