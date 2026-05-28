//! View for `flutter-cli test`. Shows a live counter + lists of running / done
//! tests + a scrollable failures pane. Survives test completion so the
//! user can read failures before pressing `q`.

use crate::theme::Theme;
use crate::view::View;
use fl_core::{KeyEvent as FlKey, TestEvent, TestResult};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::time::{Duration, Instant};

/// Transient feedback shown over the bottom of the screen — same idea
/// as `flutter-cli run`'s banner system. Auto-expires after `duration`.
#[derive(Debug, Clone, Copy)]
pub enum TestBannerKind {
    Success,
    Info,
}

#[derive(Debug, Clone)]
pub struct TestBanner {
    pub kind: TestBannerKind,
    pub message: String,
    pub shown_at: Instant,
    pub duration: Duration,
}

/// Which side of the body the keyboard / wheel scroll lands in.
/// `Tab` cycles. Visible cursor in the panel titles signals the
/// focused side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollFocus {
    Tests,
    Failures,
}

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
    /// Tests currently in flight. We display the most-recent N to keep
    /// the panel readable when flutter parallelises across many files.
    pub running: Vec<(u64, String)>,
    /// Last completed tests in chronological order, ring-buffered.
    pub recent_done: Vec<(String, TestResult, u64)>,
    pub failures: Vec<TestFailure>,
    pub all_done: bool,
    pub success: bool,
    pub quitting: bool,
    /// Set by `r` keypress. The outer driver detects this, kills the
    /// running `flutter test` subprocess, resets the view, and spawns
    /// a fresh test session. We piggy-back on `quitting` so the inner
    /// `run_view` loop returns cleanly first.
    pub wants_restart: bool,
    /// Wall-clock start of the test run, used for the chronometer.
    pub started_at: Instant,
    /// Frozen at AllDone so the final summary stops counting.
    pub finished_at: Option<Instant>,
    /// Which panel receives scroll keys / wheel. `Tab` cycles.
    pub scroll_focus: ScrollFocus,
    /// User scroll offset into `recent_done`. 0 = tail (auto-follow
    /// newest), >0 = paused N rows from latest. Bumped on each new
    /// completed test when >0 so the view stays anchored on the
    /// rows the user is reading.
    pub tests_scroll: usize,
    /// User scroll offset into `failures`. 0 = bottom (latest), larger
    /// = scrolled up into history.
    pub failure_scroll: usize,
    /// Last observed visible-row capacity of each panel (filled in
    /// each render). PgUp/PgDn use these so a page moves one real
    /// viewport.
    tests_viewport: std::sync::atomic::AtomicUsize,
    failures_viewport: std::sync::atomic::AtomicUsize,
    /// Spinner phase, incremented every tick for the running-test bullet.
    spinner_tick: u8,
    /// Test names keyed by their `id`. Flutter's `--machine` protocol
    /// only sends the test name in `testStart`; the matching `testDone`
    /// only carries the id, so we have to remember it here.
    names: std::collections::HashMap<u64, String>,
    /// Optional transient banner (e.g. "📋 Copied N failures"). Auto
    /// fades after ~3 s via `tick`.
    pub banner: Option<TestBanner>,
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
            wants_restart: false,
            started_at: Instant::now(),
            finished_at: None,
            failure_scroll: 0,
            spinner_tick: 0,
            names: std::collections::HashMap::new(),
            banner: None,
            scroll_focus: ScrollFocus::Tests,
            tests_scroll: 0,
            tests_viewport: std::sync::atomic::AtomicUsize::new(10),
            failures_viewport: std::sync::atomic::AtomicUsize::new(10),
        }
    }

    fn total(&self) -> u32 {
        self.passed + self.failed + self.skipped
    }

    /// Visible-row capacity of whichever panel currently has scroll
    /// focus. Used by PgUp/PgDn to move one real screenful.
    fn viewport_for_focus(&self) -> usize {
        use std::sync::atomic::Ordering;
        match self.scroll_focus {
            ScrollFocus::Tests => self.tests_viewport.load(Ordering::Relaxed),
            ScrollFocus::Failures => self.failures_viewport.load(Ordering::Relaxed),
        }
    }

    fn scroll_up(&mut self, n: usize) {
        match self.scroll_focus {
            ScrollFocus::Tests => {
                let max = self.recent_done.len().saturating_sub(1);
                self.tests_scroll = (self.tests_scroll + n).min(max);
            }
            ScrollFocus::Failures => {
                let max = self.failures.len().saturating_sub(1);
                self.failure_scroll = (self.failure_scroll + n).min(max);
            }
        }
    }

    fn scroll_down(&mut self, n: usize) {
        match self.scroll_focus {
            ScrollFocus::Tests => {
                self.tests_scroll = self.tests_scroll.saturating_sub(n);
            }
            ScrollFocus::Failures => {
                self.failure_scroll = self.failure_scroll.saturating_sub(n);
            }
        }
    }

    fn show_banner(&mut self, kind: TestBannerKind, message: impl Into<String>) {
        self.banner = Some(TestBanner {
            kind,
            message: message.into(),
            shown_at: Instant::now(),
            duration: Duration::from_millis(2500),
        });
    }

    fn expire_banner(&mut self) {
        if let Some(b) = &self.banner {
            if b.shown_at.elapsed() >= b.duration {
                self.banner = None;
            }
        }
    }

    fn elapsed(&self) -> Duration {
        self.finished_at
            .unwrap_or_else(Instant::now)
            .duration_since(self.started_at)
    }

    fn copy_failures_to_clipboard(&self) -> std::io::Result<usize> {
        let mut body = String::new();
        for f in &self.failures {
            body.push_str("✗ ");
            body.push_str(&f.name);
            body.push('\n');
            body.push_str("  ");
            body.push_str(&f.message);
            body.push('\n');
            if let Some(stack) = &f.stack {
                for line in stack.lines() {
                    body.push_str("    ");
                    body.push_str(line);
                    body.push('\n');
                }
            }
            body.push('\n');
        }
        // Cross-platform clipboard via arboard: macOS / Linux / Windows.
        // Replaces the previous `Command::new("pbcopy")` which ENOENT'd
        // outside macOS.
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|e| std::io::Error::other(format!("clipboard unavailable: {e}")))?;
        clipboard
            .set_text(&body)
            .map_err(|e| std::io::Error::other(format!("clipboard write failed: {e}")))?;
        Ok(self.failures.len())
    }
}

fn format_elapsed(d: Duration) -> String {
    let total = d.as_secs();
    let m = total / 60;
    let s = total % 60;
    let cs = (d.subsec_millis() / 100) as u64;
    if m > 0 {
        format!("{m:02}:{s:02}.{cs}")
    } else {
        format!("{s:02}.{cs}s")
    }
}

impl View for TestView {
    type Input = TestEvent;

    fn apply(&mut self, input: Self::Input) {
        match input {
            TestEvent::TestStarted { id, name } => {
                self.names.insert(id, name.clone());
                self.running.push((id, name));
            }
            TestEvent::TestDone {
                id,
                name,
                result,
                duration_ms,
            } => {
                self.running.retain(|(rid, _)| *rid != id);
                match result {
                    TestResult::Success => self.passed += 1,
                    TestResult::Failure | TestResult::Error => self.failed += 1,
                    TestResult::Skipped => self.skipped += 1,
                }
                // Flutter's `testDone` event doesn't actually carry the
                // test name — only the id. Fall back to the name we
                // captured at `testStart` time.
                let resolved = if !name.is_empty() {
                    name
                } else {
                    self.names
                        .remove(&id)
                        .unwrap_or_else(|| format!("test #{id}"))
                };
                self.recent_done.push((resolved, result, duration_ms));
                // Cap the ring at 200 so a 5000-test run doesn't make
                // ratatui's Paragraph allocate a huge Vec<Line> each
                // frame. The user can copy the failures separately.
                if self.recent_done.len() > 200 {
                    self.recent_done.remove(0);
                    // If the user was scrolled into history, the pop
                    // shifted indices by 1 — but our push then bumps
                    // offset back below, so net we stay anchored on
                    // the same logical rows.
                    if self.tests_scroll > 0 {
                        self.tests_scroll = self.tests_scroll.saturating_sub(1);
                    }
                }
                // Keep the scrolled-into-history user pinned to the
                // rows they were reading even as new tests arrive.
                if self.tests_scroll > 0 {
                    let cap = self.recent_done.len().saturating_sub(1);
                    self.tests_scroll = (self.tests_scroll + 1).min(cap);
                }
            }
            TestEvent::Error { id, message, stack } => {
                // Prefer the explicit testID if Flutter gave us one,
                // and fall back to "currently last-started test".
                let name = id
                    .and_then(|i| self.names.get(&i).cloned())
                    .or_else(|| self.running.last().map(|(_, n)| n.clone()))
                    .unwrap_or_else(|| "<unknown>".into());
                self.failures.push(TestFailure {
                    name,
                    message,
                    stack,
                });
            }
            TestEvent::AllDone { success, .. } => {
                self.all_done = true;
                self.success = success;
                self.finished_at = Some(Instant::now());
                // Crucially: do NOT set `quitting = true` here. The
                // view stays alive so the user can read failures /
                // scroll / copy them. They quit with `q` or Ctrl-C.
            }
            TestEvent::SuiteStart { .. } => {}
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // header
                Constraint::Min(6),    // body (live + failures)
                Constraint::Length(1), // footer
            ])
            .split(area);

        render_header(layout[0], buf, self, theme);
        render_body(layout[1], buf, self, theme);
        render_footer(layout[2], buf, self, theme);
        if let Some(b) = &self.banner {
            render_banner(area, buf, b, theme);
        }
    }

    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input> {
        match key {
            FlKey::Char('q') | FlKey::Ctrl('c') => self.quitting = true,
            FlKey::Char('r') => {
                // Re-run the whole test suite from scratch. We set
                // both flags: `quitting` so the inner `run_view` loop
                // returns control to `test_cmd::run`, and
                // `wants_restart` so it knows to respawn rather than
                // exit the process.
                self.wants_restart = true;
                self.quitting = true;
                self.show_banner(TestBannerKind::Info, "🔄 Restarting tests…");
            }
            FlKey::Tab => {
                self.scroll_focus = match self.scroll_focus {
                    ScrollFocus::Tests => ScrollFocus::Failures,
                    ScrollFocus::Failures => ScrollFocus::Tests,
                };
            }
            FlKey::Up => self.scroll_up(1),
            FlKey::Down => self.scroll_down(1),
            FlKey::PageUp => {
                let step = self.viewport_for_focus().max(1);
                self.scroll_up(step);
            }
            FlKey::PageDown => {
                let step = self.viewport_for_focus().max(1);
                self.scroll_down(step);
            }
            FlKey::Char('g') => match self.scroll_focus {
                ScrollFocus::Tests => self.tests_scroll = 0,
                ScrollFocus::Failures => self.failure_scroll = 0,
            },
            FlKey::Char('G') => match self.scroll_focus {
                ScrollFocus::Tests => {
                    self.tests_scroll = self.recent_done.len().saturating_sub(1);
                }
                ScrollFocus::Failures => {
                    self.failure_scroll = self.failures.len().saturating_sub(1);
                }
            },
            FlKey::Char('c') => {
                if self.failures.is_empty() {
                    self.show_banner(TestBannerKind::Info, "Nothing to copy — no failures 🎉");
                } else {
                    match self.copy_failures_to_clipboard() {
                        Ok(n) => self.show_banner(
                            TestBannerKind::Success,
                            format!("📋 Copied {n} failure(s) to clipboard"),
                        ),
                        Err(e) => {
                            self.show_banner(TestBannerKind::Info, format!("Copy failed: {e}"))
                        }
                    }
                }
            }
            _ => {}
        }
        None
    }

    fn tick(&mut self, _dt: Duration) {
        self.spinner_tick = self.spinner_tick.wrapping_add(1);
        self.expire_banner();
    }

    fn quitting(&self) -> bool {
        self.quitting
    }
}

fn render_header(area: Rect, buf: &mut Buffer, v: &TestView, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.dimmed())
        .style(theme.base());
    let inner = block.inner(area);
    block.render(area, buf);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(20), Constraint::Length(28)])
        .split(inner);

    // Left: counters with coloured digits so the eye lands on the
    // important number (failures, if any).
    let mut spans = vec![Span::styled(
        " flutter-cli test ── ".to_string(),
        Style::default()
            .fg(theme.accent)
            .bg(theme.bg)
            .add_modifier(Modifier::BOLD),
    )];
    spans.push(Span::styled(
        format!("✓ {}", v.passed),
        Style::default().fg(theme.success).bg(theme.bg),
    ));
    spans.push(Span::styled("  ".to_string(), theme.base()));
    spans.push(Span::styled(
        format!("✗ {}", v.failed),
        Style::default()
            .fg(if v.failed > 0 { theme.error } else { theme.dim })
            .bg(theme.bg),
    ));
    spans.push(Span::styled("  ".to_string(), theme.base()));
    spans.push(Span::styled(
        format!("– {}", v.skipped),
        Style::default().fg(theme.dim).bg(theme.bg),
    ));
    spans.push(Span::styled(
        format!("    total {}", v.total()),
        theme.dimmed(),
    ));
    Paragraph::new(Line::from(spans)).render(cols[0], buf);

    // Right: chronometer + status word.
    let status = if v.all_done {
        if v.success {
            ("✓", theme.success)
        } else {
            ("✗", theme.error)
        }
    } else {
        ("⏱", theme.fg)
    };
    let chrono = format!("{} {} ", status.0, format_elapsed(v.elapsed()));
    Paragraph::new(Line::styled(
        chrono,
        Style::default()
            .fg(status.1)
            .bg(theme.bg)
            .add_modifier(Modifier::BOLD),
    ))
    .alignment(ratatui::layout::Alignment::Right)
    .render(cols[1], buf);
}

fn render_body(area: Rect, buf: &mut Buffer, v: &TestView, theme: &Theme) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    render_live_panel(cols[0], buf, v, theme);
    render_failures_panel(cols[1], buf, v, theme);
}

fn render_live_panel(area: Rect, buf: &mut Buffer, v: &TestView, theme: &Theme) {
    let focused = matches!(v.scroll_focus, ScrollFocus::Tests);
    let focus_mark = if focused { "▸ " } else { "" };
    let scroll_hint = if v.tests_scroll > 0 {
        format!(" · paused -{}", v.tests_scroll)
    } else {
        String::new()
    };
    let title = if v.all_done {
        format!(" {focus_mark}Tests · {} done{scroll_hint} ", v.total())
    } else if v.running.is_empty() {
        format!(" {focus_mark}Tests{scroll_hint} ")
    } else {
        format!(
            " {focus_mark}Tests · {} running{scroll_hint} ",
            v.running.len()
        )
    };
    let border_style = if focused {
        Style::default().fg(theme.accent).bg(theme.bg)
    } else {
        theme.dimmed()
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(theme.base());
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 {
        return;
    }
    v.tests_viewport
        .store(inner.height as usize, std::sync::atomic::Ordering::Relaxed);

    let spinner_frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let spinner = spinner_frames[(v.spinner_tick as usize) % spinner_frames.len()];

    let mut lines: Vec<Line> = Vec::new();

    // Running tests first, with a spinner. Only when we're auto-tailing
    // (scroll == 0) — otherwise the user is reading history and we
    // shouldn't shove the running list in their face.
    if v.tests_scroll == 0 {
        for (_id, name) in v.running.iter().take(5) {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{spinner} "),
                    Style::default().fg(theme.warn).bg(theme.bg),
                ),
                Span::styled(truncate_pretty(name, area.width as usize), theme.base()),
            ]));
        }
        if v.running.len() > 5 {
            lines.push(Line::styled(
                format!("    … +{} more running", v.running.len() - 5),
                theme.dimmed(),
            ));
        }
    }

    // Then the completed tests window — the scroll offset slides the
    // window backward into history. Mirrors the logs panel in flutter-cli run.
    let budget = (inner.height as usize).saturating_sub(lines.len());
    let n = v.recent_done.len();
    let off = v.tests_scroll.min(n.saturating_sub(budget));
    let end = n.saturating_sub(off);
    let start = end.saturating_sub(budget);
    for (name, result, duration_ms) in &v.recent_done[start..end] {
        let (marker, color) = match result {
            TestResult::Success => ("✓", theme.success),
            TestResult::Failure | TestResult::Error => ("✗", theme.error),
            TestResult::Skipped => ("–", theme.dim),
        };
        let dur = format!("{}ms", duration_ms);
        lines.push(Line::from(vec![
            Span::styled(
                format!("{marker} "),
                Style::default().fg(color).bg(theme.bg),
            ),
            Span::styled(
                truncate_pretty(name, (area.width as usize).saturating_sub(dur.len() + 3)),
                theme.base(),
            ),
            Span::styled(format!("  {dur}"), theme.dimmed()),
        ]));
    }

    Paragraph::new(lines).render(inner, buf);
}

fn render_failures_panel(area: Rect, buf: &mut Buffer, v: &TestView, theme: &Theme) {
    let n = v.failures.len();
    let focused = matches!(v.scroll_focus, ScrollFocus::Failures);
    let focus_mark = if focused { "▸ " } else { "" };
    let title = if n == 0 {
        format!(" {focus_mark}Failures · none 🎉 ")
    } else if v.failure_scroll > 0 {
        format!(
            " {focus_mark}Failures · {} (paused -{}, g=tail, G=top) ",
            n, v.failure_scroll
        )
    } else {
        format!(" {focus_mark}Failures · {n} ")
    };
    let border = if focused {
        Style::default().fg(theme.accent).bg(theme.bg)
    } else if n == 0 {
        theme.dimmed()
    } else {
        Style::default().fg(theme.error).bg(theme.bg)
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border)
        .style(theme.base());
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 {
        return;
    }
    v.failures_viewport
        .store(inner.height as usize, std::sync::atomic::Ordering::Relaxed);

    if v.failures.is_empty() {
        let msg = if v.all_done && v.success {
            "All tests passed."
        } else if v.all_done {
            "Run finished without recorded failures (but exit code says fail — check daemon output)."
        } else {
            "(none yet)"
        };
        Paragraph::new(Line::styled(msg, theme.dimmed())).render(inner, buf);
        return;
    }

    // Each failure renders as 2 lines (header + message) or 2+stack
    // lines. We pack from the bottom of the visible window upward to
    // keep the latest failure in view by default.
    let max_off = n.saturating_sub(1);
    let off = v.failure_scroll.min(max_off);
    // Start at index `n - 1 - off` and walk backward filling rows.
    let start_idx = (n - 1 - off) as isize;
    let mut lines: Vec<Line> = Vec::new();
    let max_rows = inner.height as usize;
    let mut idx = start_idx;
    while idx >= 0 && lines.len() < max_rows {
        let f = &v.failures[idx as usize];
        let mut block_lines: Vec<Line> = Vec::new();
        block_lines.push(Line::from(vec![Span::styled(
            format!("✗ {}", truncate_pretty(&f.name, area.width as usize)),
            Style::default()
                .fg(theme.error)
                .bg(theme.bg)
                .add_modifier(Modifier::BOLD),
        )]));
        for msg_line in f.message.lines().take(3) {
            block_lines.push(Line::styled(
                format!("    {}", truncate_pretty(msg_line, area.width as usize)),
                theme.dimmed(),
            ));
        }
        block_lines.push(Line::styled(String::new(), theme.base()));
        // Prepend so chronological order is preserved when packing
        // bottom-up.
        let mut combined = block_lines;
        combined.extend(lines);
        lines = combined;
        idx -= 1;
    }
    if lines.len() > max_rows {
        lines.drain(..(lines.len() - max_rows));
    }
    Paragraph::new(lines).render(inner, buf);
}

fn render_footer(area: Rect, buf: &mut Buffer, v: &TestView, theme: &Theme) {
    let elapsed = format_elapsed(v.elapsed());
    let line = if v.all_done {
        let banner = if v.success {
            Span::styled(
                format!(" ✓ ALL PASSED · {} tests in {} ", v.total(), elapsed),
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.success)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                format!(" ✗ {} FAILED of {} · {} ", v.failed, v.total(), elapsed),
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.error)
                    .add_modifier(Modifier::BOLD),
            )
        };
        let keys = Span::styled(
            "  [Tab] switch  [r] re-run 🔄  [c] copy 📋  [q] quit ".to_string(),
            theme.dimmed(),
        );
        Line::from(vec![banner, keys])
    } else {
        Line::styled(
            format!(
                " ⏳ running… {} done · [Tab] switch  [r] re-run 🔄  [c] copy 📋  [q] quit ",
                v.total()
            ),
            theme.dimmed(),
        )
    };
    Paragraph::new(line).render(area, buf);
}

/// Overlay a coloured one-line banner near the top of the view —
/// matches `AppState`'s `render_banner` behaviour so the two TUIs
/// feel consistent (copy in `flutter-cli run` and copy in `flutter-cli test` look the
/// same).
fn render_banner(area: Rect, buf: &mut Buffer, banner: &TestBanner, theme: &Theme) {
    let bg = match banner.kind {
        TestBannerKind::Success => theme.success,
        TestBannerKind::Info => theme.cyan,
    };
    let body = format!(" {} ", banner.message);
    let width = body.chars().count() as u16;
    let target = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + 1,
        width,
        height: 1,
    };
    Paragraph::new(Line::styled(
        body,
        Style::default()
            .fg(theme.bg)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    ))
    .render(target, buf);
}

/// Cap a string at `max` display columns, appending `…` when truncated.
fn truncate_pretty(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 1 {
        return "…".to_string();
    }
    s.chars().take(max - 1).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passed_increments_on_success() {
        let mut v = TestView::new();
        v.apply(TestEvent::TestStarted {
            id: 1,
            name: "t".into(),
        });
        v.apply(TestEvent::TestDone {
            id: 1,
            name: "t".into(),
            result: TestResult::Success,
            duration_ms: 10,
        });
        assert_eq!(v.passed, 1);
        assert!(v.running.is_empty());
    }

    #[test]
    fn failed_increments_on_failure_and_records_failure() {
        let mut v = TestView::new();
        v.apply(TestEvent::TestStarted {
            id: 2,
            name: "t2".into(),
        });
        v.apply(TestEvent::Error {
            id: Some(2),
            message: "boom".into(),
            stack: None,
        });
        v.apply(TestEvent::TestDone {
            id: 2,
            name: "t2".into(),
            result: TestResult::Failure,
            duration_ms: 5,
        });
        assert_eq!(v.failed, 1);
        assert_eq!(v.failures.len(), 1);
        assert!(v.failures[0].message.contains("boom"));
    }

    #[test]
    fn all_done_does_not_quit_so_user_can_read_failures() {
        let mut v = TestView::new();
        v.apply(TestEvent::AllDone {
            success: false,
            passed: 0,
            failed: 0,
            skipped: 0,
        });
        assert!(v.all_done);
        assert!(!v.quitting, "view must stay alive after AllDone");
        // … until the user presses q.
        v.handle_key(FlKey::Char('q'));
        assert!(v.quitting);
    }

    #[test]
    fn pressing_c_with_no_failures_shows_nothing_to_copy_banner() {
        let mut v = TestView::new();
        assert!(v.banner.is_none());
        v.handle_key(FlKey::Char('c'));
        let b = v.banner.as_ref().expect("banner should be set");
        assert!(b.message.contains("Nothing to copy"));
    }

    #[test]
    fn banner_expires_after_its_duration() {
        let mut v = TestView::new();
        v.show_banner(TestBannerKind::Info, "hi");
        assert!(v.banner.is_some());
        // Force the shown_at into the past beyond the configured
        // duration, then tick to trigger expiry.
        if let Some(b) = v.banner.as_mut() {
            b.shown_at = Instant::now() - Duration::from_secs(10);
        }
        v.tick(Duration::from_millis(33));
        assert!(v.banner.is_none(), "banner should auto-expire");
    }

    #[test]
    fn pressing_r_sets_both_quitting_and_wants_restart() {
        let mut v = TestView::new();
        v.apply(TestEvent::AllDone {
            success: true,
            passed: 1,
            failed: 0,
            skipped: 0,
        });
        // Pressing `q` would just quit.
        // Pressing `r` should ALSO quit (so the run loop returns) but
        // set wants_restart so the driver respawns the test process.
        v.handle_key(FlKey::Char('r'));
        assert!(v.quitting);
        assert!(v.wants_restart);
    }

    #[test]
    fn failure_scroll_clamps_at_boundaries() {
        let mut v = TestView::new();
        // Switch focus to failures so Up/Down target that side.
        v.handle_key(FlKey::Tab);
        assert_eq!(v.scroll_focus, ScrollFocus::Failures);
        v.failures.push(TestFailure {
            name: "a".into(),
            message: "x".into(),
            stack: None,
        });
        v.failures.push(TestFailure {
            name: "b".into(),
            message: "y".into(),
            stack: None,
        });
        v.handle_key(FlKey::Up);
        v.handle_key(FlKey::Up);
        v.handle_key(FlKey::Up);
        assert_eq!(v.failure_scroll, 1, "should clamp at len-1");
        v.handle_key(FlKey::Down);
        v.handle_key(FlKey::Down);
        v.handle_key(FlKey::Down);
        assert_eq!(v.failure_scroll, 0, "should saturate at 0");
    }

    #[test]
    fn tests_scroll_keeps_anchor_when_new_tests_complete() {
        let mut v = TestView::new();
        for i in 0..5 {
            v.apply(TestEvent::TestStarted {
                id: i,
                name: format!("t{i}"),
            });
            v.apply(TestEvent::TestDone {
                id: i,
                name: format!("t{i}"),
                result: TestResult::Success,
                duration_ms: 1,
            });
        }
        // Scroll back two rows.
        v.handle_key(FlKey::Up);
        v.handle_key(FlKey::Up);
        assert_eq!(v.tests_scroll, 2);
        // New tests come in — offset should bump so we stay anchored.
        v.apply(TestEvent::TestStarted {
            id: 99,
            name: "t99".into(),
        });
        v.apply(TestEvent::TestDone {
            id: 99,
            name: "t99".into(),
            result: TestResult::Success,
            duration_ms: 1,
        });
        assert_eq!(v.tests_scroll, 3, "offset bumped to keep view stable");
    }

    #[test]
    fn tab_cycles_scroll_focus_between_tests_and_failures() {
        let mut v = TestView::new();
        assert_eq!(v.scroll_focus, ScrollFocus::Tests);
        v.handle_key(FlKey::Tab);
        assert_eq!(v.scroll_focus, ScrollFocus::Failures);
        v.handle_key(FlKey::Tab);
        assert_eq!(v.scroll_focus, ScrollFocus::Tests);
    }
}
