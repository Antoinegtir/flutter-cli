//! View for `fl build <target>` — phase list + final binary report.

use crate::theme::Theme;
use crate::view::View;
use fl_core::{BuildMode, BuildTarget, FlutterEvent, KeyEvent as FlKey, LogLevel};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct BuildStep {
    pub id: String,
    pub message: String,
    pub status: StepStatus,
    pub started_at: Instant,
    pub finished_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Running,
    Done,
    Failed,
}

pub struct BuildView {
    pub target: BuildTarget,
    pub mode: BuildMode,
    pub steps: Vec<BuildStep>,
    pub log_tail: Vec<String>,
    pub final_size: Option<u64>,
    pub final_path: Option<String>,
    pub quitting: bool,
    pub started_at: Instant,
    pub elapsed_ms: u64,
}

impl BuildView {
    pub fn new(target: BuildTarget, mode: BuildMode) -> Self {
        Self {
            target,
            mode,
            steps: Vec::new(),
            log_tail: Vec::new(),
            final_size: None,
            final_path: None,
            quitting: false,
            started_at: Instant::now(),
            elapsed_ms: 0,
        }
    }
}

impl View for BuildView {
    type Input = FlutterEvent;

    fn apply(&mut self, input: Self::Input) {
        match input {
            FlutterEvent::Progress { id, message, finished } => {
                if let Some(existing) = self.steps.iter_mut().find(|s| s.id == id) {
                    existing.message = message;
                    if finished && existing.status == StepStatus::Running {
                        existing.status = StepStatus::Done;
                        existing.finished_at = Some(Instant::now());
                    }
                } else {
                    self.steps.push(BuildStep {
                        id,
                        message,
                        status: if finished { StepStatus::Done } else { StepStatus::Running },
                        started_at: Instant::now(),
                        finished_at: if finished { Some(Instant::now()) } else { None },
                    });
                }
            }
            FlutterEvent::Log { level, message } => {
                if matches!(level, LogLevel::Error) {
                    if let Some(last) = self.steps.last_mut() {
                        if last.status == StepStatus::Running {
                            last.status = StepStatus::Failed;
                            last.finished_at = Some(Instant::now());
                        }
                    }
                }
                // Detect the final "Built <path> (NN.NMB)" line.
                if let Some(rest) = message.strip_prefix("Built ") {
                    if let Some((path, size)) = parse_built_line(rest) {
                        self.final_path = Some(path);
                        self.final_size = Some(size);
                    }
                }
                self.log_tail.push(message);
                if self.log_tail.len() > 200 {
                    self.log_tail.remove(0);
                }
            }
            FlutterEvent::Stopped { exit_code } => {
                self.quitting = true;
                if let Some(code) = exit_code {
                    if code != 0 {
                        if let Some(last) = self.steps.last_mut() {
                            if last.status == StepStatus::Running {
                                last.status = StepStatus::Failed;
                                last.finished_at = Some(Instant::now());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(3),
            ])
            .split(area);

        // Header
        let header_text = format!(
            " fl build ── {} · {} · {:>4}.{}s ",
            self.target.flutter_arg(),
            mode_label(self.mode),
            self.elapsed_ms / 1000,
            self.elapsed_ms % 1000 / 100
        );
        let header_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent).bg(theme.bg))
            .style(theme.base());
        let header_inner = header_block.inner(layout[0]);
        header_block.render(layout[0], buf);
        Paragraph::new(Line::styled(header_text, theme.header())).render(header_inner, buf);

        // Steps
        let steps_block = Block::default()
            .title(" Steps ")
            .borders(Borders::ALL)
            .border_style(theme.dimmed())
            .style(theme.base());
        let steps_inner = steps_block.inner(layout[1]);
        steps_block.render(layout[1], buf);

        let mut lines: Vec<Line> = Vec::new();
        for step in &self.steps {
            let (marker, color) = match step.status {
                StepStatus::Running => ("⠋ ", theme.warn),
                StepStatus::Done => ("✓ ", theme.success),
                StepStatus::Failed => ("✗ ", theme.error),
            };
            let elapsed_ms = step
                .finished_at
                .unwrap_or_else(Instant::now)
                .duration_since(step.started_at)
                .as_millis();
            lines.push(Line::styled(
                format!("{marker}{:<40} {:>5}ms", step.message, elapsed_ms),
                Style::default().fg(color).bg(theme.bg),
            ));
        }
        Paragraph::new(lines).render(steps_inner, buf);

        // Footer (final size / status)
        let footer_block = Block::default().borders(Borders::ALL).border_style(theme.dimmed()).style(theme.base());
        let footer_inner = footer_block.inner(layout[2]);
        footer_block.render(layout[2], buf);
        let footer_text = match (&self.final_path, self.final_size) {
            (Some(path), Some(size)) => format!("Built {path} · {}", human_size(size)),
            _ => " ".to_string(),
        };
        Paragraph::new(Line::styled(footer_text, theme.dimmed())).render(footer_inner, buf);
    }

    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input> {
        if matches!(key, FlKey::Char('q') | FlKey::Ctrl('c')) {
            self.quitting = true;
        }
        None
    }

    fn tick(&mut self, dt: Duration) {
        self.elapsed_ms = self.elapsed_ms.saturating_add(dt.as_millis() as u64);
    }

    fn quitting(&self) -> bool {
        self.quitting
    }
}

fn mode_label(m: BuildMode) -> &'static str {
    match m {
        BuildMode::Debug => "debug",
        BuildMode::Profile => "profile",
        BuildMode::Release => "release",
    }
}

fn parse_built_line(rest: &str) -> Option<(String, u64)> {
    // "build/app/outputs/flutter-apk/app-release.apk (12.3MB)."
    let (path, size_part) = rest.rsplit_once(" (")?;
    let size_str = size_part.trim_end_matches(").").trim_end_matches(')');
    let bytes = parse_size_to_bytes(size_str)?;
    Some((path.to_string(), bytes))
}

fn parse_size_to_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| c.is_alphabetic())?);
    let value: f64 = num.trim().parse().ok()?;
    let mult: u64 = match unit.trim() {
        "B" | "b" => 1,
        "KB" | "kB" => 1024,
        "MB" | "mB" => 1024 * 1024,
        "GB" | "gB" => 1024 * 1024 * 1024,
        _ => return None,
    };
    Some((value * mult as f64) as u64)
}

fn human_size(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1}{}", units[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_progress_event_starts_a_step() {
        let mut v = BuildView::new(BuildTarget::Apk, BuildMode::Release);
        v.apply(FlutterEvent::Progress {
            id: "gradle".into(),
            message: "Running Gradle task".into(),
            finished: false,
        });
        assert_eq!(v.steps.len(), 1);
        assert_eq!(v.steps[0].status, StepStatus::Running);
    }

    #[test]
    fn progress_with_finished_marks_step_done() {
        let mut v = BuildView::new(BuildTarget::Apk, BuildMode::Release);
        v.apply(FlutterEvent::Progress { id: "g".into(), message: "x".into(), finished: false });
        v.apply(FlutterEvent::Progress { id: "g".into(), message: "x done".into(), finished: true });
        assert_eq!(v.steps[0].status, StepStatus::Done);
    }

    #[test]
    fn captures_final_binary_size_from_log() {
        let mut v = BuildView::new(BuildTarget::Apk, BuildMode::Release);
        v.apply(FlutterEvent::Log {
            level: LogLevel::Info,
            message: "Built build/app/outputs/flutter-apk/app-release.apk (12.3MB).".into(),
        });
        assert_eq!(v.final_size, Some((12.3 * 1024.0 * 1024.0) as u64));
        assert!(v.final_path.as_ref().unwrap().contains("app-release.apk"));
    }

    #[test]
    fn stopped_marks_running_step_failed_on_nonzero_exit() {
        let mut v = BuildView::new(BuildTarget::Apk, BuildMode::Release);
        v.apply(FlutterEvent::Progress { id: "g".into(), message: "x".into(), finished: false });
        v.apply(FlutterEvent::Stopped { exit_code: Some(1) });
        assert_eq!(v.steps[0].status, StepStatus::Failed);
        assert!(v.quitting);
    }
}
