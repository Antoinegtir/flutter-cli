//! View for `fl test`.

use crate::theme::Theme;
use crate::view::View;
use fl_core::{KeyEvent as FlKey, TestEvent, TestResult};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct TestFailure {
    pub name: String,
    pub message: String,
    pub stack: Option<String>,
}

pub struct TestView {
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub running: Vec<(u64, String)>,
    pub recent_done: Vec<(String, TestResult)>,
    pub failures: Vec<TestFailure>,
    pub all_done: bool,
    pub success: bool,
    pub quitting: bool,
}

impl Default for TestView {
    fn default() -> Self {
        Self::new()
    }
}

impl TestView {
    pub fn new() -> Self {
        Self {
            passed: 0,
            failed: 0,
            skipped: 0,
            running: Vec::new(),
            recent_done: Vec::new(),
            failures: Vec::new(),
            all_done: false,
            success: false,
            quitting: false,
        }
    }
}

impl View for TestView {
    type Input = TestEvent;

    fn apply(&mut self, input: Self::Input) {
        match input {
            TestEvent::TestStarted { id, name } => {
                self.running.push((id, name));
            }
            TestEvent::TestDone { id, name, result, duration_ms: _ } => {
                self.running.retain(|(rid, _)| *rid != id);
                match result {
                    TestResult::Success => self.passed += 1,
                    TestResult::Failure => self.failed += 1,
                    TestResult::Error => self.failed += 1,
                    TestResult::Skipped => self.skipped += 1,
                }
                self.recent_done.push((name, result));
                if self.recent_done.len() > 20 {
                    self.recent_done.remove(0);
                }
            }
            TestEvent::Error { id: _, message, stack } => {
                let name = self.running.last().map(|(_, n)| n.clone()).unwrap_or_else(|| "<unknown>".into());
                self.failures.push(TestFailure { name, message, stack });
            }
            TestEvent::AllDone { success, .. } => {
                self.all_done = true;
                self.success = success;
                self.quitting = true;
            }
            TestEvent::SuiteStart { .. } => {}
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(8),
            ])
            .split(area);

        // Header: big counter
        let header_block = Block::default().borders(Borders::ALL).border_style(theme.dimmed()).style(theme.base());
        let inner = header_block.inner(layout[0]);
        header_block.render(layout[0], buf);
        let counter = format!(
            " fl test ── ✓ {}  ✗ {}  – {}",
            self.passed, self.failed, self.skipped
        );
        let color = if self.failed > 0 { theme.error } else { theme.success };
        Paragraph::new(Line::styled(counter, Style::default().fg(color).bg(theme.bg))).render(inner, buf);

        // Live list (running + recent)
        let live_block = Block::default().title(" Live ").borders(Borders::ALL).border_style(theme.dimmed()).style(theme.base());
        let inner = live_block.inner(layout[1]);
        live_block.render(layout[1], buf);
        let mut lines: Vec<Line> = Vec::new();
        for (_id, name) in &self.running {
            lines.push(Line::styled(format!("⠋ {name}"), Style::default().fg(theme.warn).bg(theme.bg)));
        }
        for (name, result) in self.recent_done.iter().rev().take(inner.height as usize).rev() {
            let (marker, color) = match result {
                TestResult::Success => ("✓", theme.success),
                TestResult::Failure => ("✗", theme.error),
                TestResult::Error => ("✗", theme.error),
                TestResult::Skipped => ("–", theme.dim),
            };
            lines.push(Line::styled(format!("{marker} {name}"), Style::default().fg(color).bg(theme.bg)));
        }
        Paragraph::new(lines).render(inner, buf);

        // Failures
        let fail_block = Block::default().title(" Failures ").borders(Borders::ALL).border_style(theme.dimmed()).style(theme.base());
        let inner = fail_block.inner(layout[2]);
        fail_block.render(layout[2], buf);
        let mut fail_lines: Vec<Line> = Vec::new();
        for f in self.failures.iter().rev().take(3).rev() {
            fail_lines.push(Line::styled(format!("✗ {}", f.name), Style::default().fg(theme.error).bg(theme.bg)));
            fail_lines.push(Line::styled(format!("    {}", f.message), theme.dimmed()));
        }
        Paragraph::new(fail_lines).render(inner, buf);
    }

    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input> {
        if matches!(key, FlKey::Char('q') | FlKey::Ctrl('c')) {
            self.quitting = true;
        }
        None
    }
    fn tick(&mut self, _dt: Duration) {}
    fn quitting(&self) -> bool {
        self.quitting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passed_increments_on_success() {
        let mut v = TestView::new();
        v.apply(TestEvent::TestStarted { id: 1, name: "t".into() });
        v.apply(TestEvent::TestDone { id: 1, name: "t".into(), result: TestResult::Success, duration_ms: 10 });
        assert_eq!(v.passed, 1);
        assert!(v.running.is_empty());
    }

    #[test]
    fn failed_increments_on_failure_and_records_failure() {
        let mut v = TestView::new();
        v.apply(TestEvent::TestStarted { id: 2, name: "t2".into() });
        v.apply(TestEvent::Error { id: Some(2), message: "boom".into(), stack: None });
        v.apply(TestEvent::TestDone { id: 2, name: "t2".into(), result: TestResult::Failure, duration_ms: 5 });
        assert_eq!(v.failed, 1);
        assert_eq!(v.failures.len(), 1);
        assert!(v.failures[0].message.contains("boom"));
    }

    #[test]
    fn all_done_sets_quitting() {
        let mut v = TestView::new();
        v.apply(TestEvent::AllDone { success: true, passed: 0, failed: 0, skipped: 0 });
        assert!(v.quitting);
    }
}
