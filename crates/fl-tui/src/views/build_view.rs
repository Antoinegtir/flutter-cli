//! View for `fl build <target>` — phase list + final binary report.

use crate::theme::Theme;
use crate::view::View;
use fl_core::{BuildMode, FlutterEvent, KeyEvent as FlKey, LogLevel};
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
    /// Whatever the user typed as the build subcommand (apk / ios /
    /// ipa / macos / web / aar / …). Stored as a free-form string so
    /// any `flutter build <target>` works, not just our short enum.
    pub target: String,
    pub mode: BuildMode,
    pub steps: Vec<BuildStep>,
    /// Each log line as it arrived — every `Log` event is appended,
    /// capped at the most recent 200 lines. Shown in the Steps panel
    /// below any parsed Progress events so the user actually sees
    /// build progression even when `flutter build` doesn't emit
    /// structured JSON (i.e. without `--machine`).
    pub log_tail: Vec<(LogLevel, String)>,
    pub final_size: Option<u64>,
    pub final_path: Option<String>,
    pub quitting: bool,
    pub started_at: Instant,
    pub elapsed_ms: u64,
    /// Exit code captured from `FlutterEvent::Stopped`. Used by the
    /// renderer to colour the header / footer red when the build
    /// failed instead of just disappearing without explanation.
    pub exit_code: Option<i32>,
    /// Accumulated tick duration since the build process exited.
    /// Drives the post-exit linger so the user gets ~2 s to read the
    /// final log lines (especially the error case where the process
    /// dies in <1 s). We accumulate via `tick(dt)` rather than read
    /// wall time so unit tests can drive the linger deterministically.
    pub linger_ms: Option<u64>,
}

impl BuildView {
    pub fn new(target: String, mode: BuildMode) -> Self {
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
            exit_code: None,
            linger_ms: None,
        }
    }
}

impl View for BuildView {
    type Input = FlutterEvent;

    fn apply(&mut self, input: Self::Input) {
        match input {
            FlutterEvent::Progress {
                id,
                message,
                finished,
            } => {
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
                        status: if finished {
                            StepStatus::Done
                        } else {
                            StepStatus::Running
                        },
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
                self.log_tail.push((level, message));
                if self.log_tail.len() > 200 {
                    self.log_tail.remove(0);
                }
            }
            FlutterEvent::Stopped { exit_code } => {
                self.exit_code = exit_code;
                self.linger_ms = Some(0);
                if exit_code.unwrap_or(0) != 0 {
                    if let Some(last) = self.steps.last_mut() {
                        if last.status == StepStatus::Running {
                            last.status = StepStatus::Failed;
                            last.finished_at = Some(Instant::now());
                        }
                    }
                }
                // On a success exit, close immediately — user got the
                // "Built …" footer line and there's no error to read.
                // On a non-zero exit, linger 2s via tick() so the user
                // can read why it failed before the box disappears.
                if exit_code.unwrap_or(0) == 0 {
                    self.quitting = true;
                }
            }
            _ => {}
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // 4 stacked regions:
        //   header   (3 rows, bordered: target/mode/elapsed)
        //   Steps    (flex,   bordered: parsed steps + log tail)
        //   status   (3 rows, bordered: built-path / exit code)
        //   keybinds (1 row,  no border:  [c] copy  [q] quit)
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area);

        // Header
        let header_text = format!(
            " fl build ── {} · {} · {:>4}.{}s ",
            self.target,
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
        // Append the log tail so the user sees real build progression
        // (and especially the error output) even when `flutter build`
        // emits plain text instead of structured `--machine` events.
        // We tail just enough lines to fill the remaining rows below
        // any parsed Steps — older lines fall off the top naturally.
        let used = self.steps.len();
        let cap = (steps_inner.height as usize).saturating_sub(used);
        if cap > 0 {
            let start = self.log_tail.len().saturating_sub(cap);
            for (level, msg) in &self.log_tail[start..] {
                // Flutter's final success line — "✓ Built <path> (NMB)."
                // or just "Built <path> (NMB)." — should pop in green
                // even though it ships as a regular Info log. Same for
                // the clipboard confirmation we inject ourselves, so
                // the user gets a quick visual ack when they hit `c`.
                let color = if msg.starts_with("✓ Built ")
                    || msg.starts_with("Built ")
                    || msg.starts_with("📋 ")
                {
                    theme.success
                } else {
                    match level {
                        LogLevel::Error => theme.error,
                        LogLevel::Warn => theme.warn,
                        LogLevel::Debug | LogLevel::Trace => theme.dim,
                        LogLevel::Info => theme.fg,
                    }
                };
                let truncated: String = msg.chars().take(steps_inner.width as usize).collect();
                lines.push(Line::styled(
                    truncated,
                    Style::default().fg(color).bg(theme.bg),
                ));
            }
        }
        Paragraph::new(lines).render(steps_inner, buf);

        // Footer (final size / status / exit code)
        let footer_block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.dimmed())
            .style(theme.base());
        let footer_inner = footer_block.inner(layout[2]);
        footer_block.render(layout[2], buf);
        let (footer_text, footer_color) = match (&self.final_path, self.final_size, self.exit_code)
        {
            (Some(path), Some(size), _) => (
                format!("Built {path} · {}", human_size(size)),
                theme.success,
            ),
            (_, _, Some(code)) if code != 0 => (
                format!("✗ flutter build exited with code {code} — see log above"),
                theme.error,
            ),
            (_, _, Some(_)) => ("✓ done".to_string(), theme.success),
            _ => (" ".to_string(), theme.dim),
        };
        Paragraph::new(Line::styled(
            footer_text,
            Style::default().fg(footer_color).bg(theme.bg),
        ))
        .render(footer_inner, buf);

        // Keybinds row — pinned at the bottom of the box.
        Paragraph::new(Line::styled(
            " [c] copy logs 📋   [q] quit ",
            theme.dimmed(),
        ))
        .render(layout[3], buf);
    }

    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input> {
        match key {
            FlKey::Char('q') | FlKey::Ctrl('c') => self.quitting = true,
            FlKey::Char('c') => {
                // Dump every log line we've collected to the system
                // clipboard. The result is reported back into log_tail
                // so the user sees confirmation inside the panel —
                // BuildView has no banner/snackbar surface of its own.
                let mut text = String::new();
                for (lvl, msg) in &self.log_tail {
                    let tag = match lvl {
                        LogLevel::Error => "ERROR",
                        LogLevel::Warn => "WARN ",
                        LogLevel::Info => "INFO ",
                        LogLevel::Debug => "DEBUG",
                        LogLevel::Trace => "TRACE",
                    };
                    text.push_str(tag);
                    text.push(' ');
                    text.push_str(msg);
                    text.push('\n');
                }
                let n = self.log_tail.len();
                let (lvl, msg) = match copy_to_clipboard(&text) {
                    Ok(()) => (
                        LogLevel::Info,
                        format!("📋 Copied {n} log lines to clipboard"),
                    ),
                    Err(e) => (LogLevel::Warn, format!("clipboard copy failed: {e}")),
                };
                self.log_tail.push((lvl, msg));
                if self.log_tail.len() > 200 {
                    self.log_tail.remove(0);
                }
            }
            _ => {}
        }
        None
    }

    fn tick(&mut self, dt: Duration) {
        self.elapsed_ms = self.elapsed_ms.saturating_add(dt.as_millis() as u64);
        // Post-failure linger: keep the dashboard on screen for 2s
        // after the process exits with an error so the user actually
        // sees the last log lines instead of the TUI flashing and
        // disappearing.
        if let Some(ms) = self.linger_ms.as_mut() {
            *ms = ms.saturating_add(dt.as_millis() as u64);
            if !self.quitting && *ms >= 2000 {
                self.quitting = true;
            }
        }
    }

    fn quitting(&self) -> bool {
        self.quitting
    }
}

/// Copy `text` to the OS clipboard via `arboard` (cross-platform).
/// Duplicated from `app.rs` rather than shared via a util module to
/// keep BuildView self-contained; the helper is short and stable.
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| std::io::Error::other(format!("clipboard unavailable: {e}")))?;
    clipboard
        .set_text(text)
        .map_err(|e| std::io::Error::other(format!("clipboard write failed: {e}")))?;
    Ok(())
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
        let mut v = BuildView::new("apk".to_string(), BuildMode::Release);
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
        let mut v = BuildView::new("apk".to_string(), BuildMode::Release);
        v.apply(FlutterEvent::Progress {
            id: "g".into(),
            message: "x".into(),
            finished: false,
        });
        v.apply(FlutterEvent::Progress {
            id: "g".into(),
            message: "x done".into(),
            finished: true,
        });
        assert_eq!(v.steps[0].status, StepStatus::Done);
    }

    #[test]
    fn captures_final_binary_size_from_log() {
        let mut v = BuildView::new("apk".to_string(), BuildMode::Release);
        v.apply(FlutterEvent::Log {
            level: LogLevel::Info,
            message: "Built build/app/outputs/flutter-apk/app-release.apk (12.3MB).".into(),
        });
        assert_eq!(v.final_size, Some((12.3 * 1024.0 * 1024.0) as u64));
        assert!(v.final_path.as_ref().unwrap().contains("app-release.apk"));
    }

    #[test]
    fn stopped_marks_running_step_failed_on_nonzero_exit() {
        let mut v = BuildView::new("apk".to_string(), BuildMode::Release);
        v.apply(FlutterEvent::Progress {
            id: "g".into(),
            message: "x".into(),
            finished: false,
        });
        v.apply(FlutterEvent::Stopped { exit_code: Some(1) });
        assert_eq!(v.steps[0].status, StepStatus::Failed);
        // On a failure exit, the view lingers ~2s before quitting so
        // the user can read the final log lines. Immediately after the
        // event it should NOT be quitting yet.
        assert!(!v.quitting);
        // Driving tick past the linger window flips quitting on.
        v.tick(Duration::from_secs(2));
        assert!(v.quitting);
    }

    #[test]
    fn stopped_with_success_exit_quits_immediately() {
        let mut v = BuildView::new("apk".to_string(), BuildMode::Release);
        v.apply(FlutterEvent::Stopped { exit_code: Some(0) });
        assert!(v.quitting);
    }
}
