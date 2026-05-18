//! View for `fl clean`.

use crate::spinner::Spinner;
use crate::theme::Theme;
use crate::view::View;
use fl_core::{CleanEvent, KeyEvent as FlKey};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::time::Duration;

pub struct CleanView {
    pub spinner: Spinner,
    pub current_path: Option<String>,
    pub paths: Vec<String>,
    pub freed_bytes: u64,
    pub done: bool,
    pub quitting: bool,
    pub error: Option<String>,
}

impl CleanView {
    pub fn new() -> Self {
        Self {
            spinner: Spinner::default(),
            current_path: None,
            paths: Vec::new(),
            freed_bytes: 0,
            done: false,
            quitting: false,
            error: None,
        }
    }
}

impl View for CleanView {
    type Input = CleanEvent;

    fn apply(&mut self, input: Self::Input) {
        match input {
            CleanEvent::Probing => self.current_path = Some("(measuring…)".into()),
            CleanEvent::Cleaning { path } => self.current_path = Some(path),
            CleanEvent::Done { freed_bytes, paths } => {
                self.freed_bytes = freed_bytes;
                self.paths = paths;
                self.done = true;
                self.quitting = true;
                self.current_path = None;
            }
            CleanEvent::Error(msg) => {
                self.error = Some(msg);
                self.quitting = true;
            }
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let block = Block::default().title(" fl clean ").borders(Borders::ALL).border_style(theme.dimmed()).style(theme.base());
        let inner = block.inner(area);
        block.render(area, buf);
        if let Some(err) = &self.error {
            Paragraph::new(Line::styled(format!("✗ {err}"), Style::default().fg(theme.error).bg(theme.bg)))
                .alignment(Alignment::Center)
                .render(inner, buf);
            return;
        }
        if self.done {
            let pretty = format!("🧹 Cleaned {}", human_size(self.freed_bytes));
            Paragraph::new(Line::styled(pretty, Style::default().fg(theme.success).bg(theme.bg)))
                .alignment(Alignment::Center)
                .render(inner, buf);
        } else {
            let line = match &self.current_path {
                Some(p) => format!("{}  {p}", self.spinner.frame()),
                None => format!("{}  Initializing…", self.spinner.frame()),
            };
            Paragraph::new(Line::styled(line, Style::default().fg(theme.warn).bg(theme.bg)))
                .alignment(Alignment::Center)
                .render(inner, buf);
        }
    }

    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input> {
        if matches!(key, FlKey::Char('q') | FlKey::Ctrl('c')) {
            self.quitting = true;
        }
        None
    }
    fn tick(&mut self, dt: Duration) { self.spinner.tick(dt); }
    fn quitting(&self) -> bool { self.quitting }
}

fn human_size(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 { v /= 1024.0; i += 1; }
    format!("{v:.1} {}", units[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probing_sets_current_path() {
        let mut v = CleanView::new();
        v.apply(CleanEvent::Probing);
        assert!(v.current_path.is_some());
    }

    #[test]
    fn done_records_freed_and_sets_quitting() {
        let mut v = CleanView::new();
        v.apply(CleanEvent::Done { freed_bytes: 1_500_000, paths: vec!["build/".into()] });
        assert!(v.quitting);
        assert_eq!(v.freed_bytes, 1_500_000);
        assert_eq!(v.paths.len(), 1);
    }

    #[test]
    fn error_records_and_quits() {
        let mut v = CleanView::new();
        v.apply(CleanEvent::Error("boom".into()));
        assert!(v.error.is_some());
        assert!(v.quitting);
    }
}
