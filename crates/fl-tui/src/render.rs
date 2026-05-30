//! Top-level dashboard render: header + body split + footer + optional banner.

use crate::app::{AppState, BannerKind};
use crate::panels;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

const FOOTER_SHORT: &str = " r reload · q quit ";

/// All keybinds in priority order — first ones survive at the
/// narrowest widths, last ones are dropped first. `render_footer`
/// walks this list and packs as many entries as fit. Two-space gap
/// between entries to match the previous fixed-tier spacing.
const FOOTER_BINDS: &[&str] = &[
    "[r] reload",
    "[R] restart",
    "[q] quit",
    "[e] error ↗",
    "[c] copy 📋",
    "[/] filter",
    "[s] snap 📸",
    "[b] theme",
    "[n] net",
    "[d] devtools",
    "[o] platform",
    "[p] paint",
    "[P] perf",
];

// Pre-ready footer suffix: kept compact so the percentage progress
// bar in `render_footer` always has room. `e` (jump to error in IDE)
// is surfaced even though the app isn't running yet — it's the
// moment the user needs it most, since most pre-ready failures are
// Dart compilation errors with a file ref in the log.
const FOOTER_FULL_PRE_READY_STATIC: &str = " [e] error ↗  [/] filter  [c] copy 📋  [q] quit ";
const FOOTER_MEDIUM_PRE_READY_STATIC: &str = " [e] err  [q] quit ";
const FOOTER_SHORT_PRE_READY_STATIC: &str = "· e err · q quit ";

const MIN_WIDTH: u16 = 50;
const MIN_HEIGHT: u16 = 8;
const NARROW_WIDTH: u16 = 90;
const HEADER_HEIGHT: u16 = 3;

pub fn render(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_too_small(area, buf, theme);
        return;
    }
    // Inline-viewport layout (Claude-Code style). Logs flow into the
    // terminal's scrollback (see `TuiRunner::print_above_viewport`) and
    // scroll naturally above the box. What remains pinned to the
    // bottom is the live status surface:
    //   1. fl-info status header   (3 rows)
    //   2. Performance + Devices   (flex)
    //   3. Footer                  (1 row — keybinds when ready, a
    //                               percentage progress bar pre-ready)
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(HEADER_HEIGHT),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(area);
    render_header(layout[0], buf, state, theme);
    render_status_panels(layout[1], buf, state, theme);
    render_footer(layout[2], buf, state, theme);
}

/// Determinate progress bar. Returns a `width`-wide string made of
/// `█` (filled cells) and `░` (empty cells), proportional to
/// `pct` ∈ [0, 100]. We use a single glyph type rather than the
/// finer-grain 1/8-block shading because partial blocks render
/// unevenly across the wide range of terminal fonts the user might
/// have (JetBrains Mono in AS shows them at half height, for example).
fn build_bar(width: usize, pct: u8) -> String {
    if width == 0 {
        return String::new();
    }
    let filled = (width as u32 * pct.min(100) as u32 / 100) as usize;
    let mut out = String::with_capacity(width);
    for _ in 0..filled {
        out.push('█');
    }
    for _ in filled..width {
        out.push('░');
    }
    out
}

/// Estimated launch progress in `[0, 100]`. Combines completed-phase
/// weights with linear interpolation inside the current phase. Caps at
/// 99 until the VM Service connects so we never show 100% before the
/// app is actually ready.
///
/// Each known Flutter startup phase contributes a fixed slice of the
/// 100% budget (`phase_weight`) and a typical duration we interpolate
/// against (`phase_estimated_secs`). Unknown phases get a default 8%
/// weight so they advance the bar by SOMETHING but don't overshoot.
fn estimated_percentage(state: &AppState) -> u8 {
    if state.app_ready() {
        return 100;
    }
    // Pre-progress: a small seed value so the bar isn't dead at 0%
    // during the first second of activity — `flutter run` takes a
    // beat to emit its first event and a fully-empty bar reads as
    // "frozen" to the user. Cap at 3% so we don't lie too much.
    if state.progress_phases.is_empty() {
        let secs = state.started_at.elapsed().as_secs();
        return secs.min(3) as u8;
    }
    let mut pct: f64 = 0.0;
    for phase in &state.progress_phases {
        let weight = phase_weight(phase) as f64;
        match phase.finished_at {
            Some(_) => pct += weight,
            None => {
                // Prefer REAL progress from observed Xcode sub-steps
                // when our patched flutter_tools emits them. Each
                // `xcode.build.line` event = one compile / link /
                // process step that actually ran, so the count is a
                // genuine measure of "work done" — not a timer guess.
                // Without a known total we use a typical-build
                // denominator (TYPICAL_XCODE_STEPS); the inside-phase
                // value still saturates at 0.95 so the bar never
                // pre-empts the daemon's closer event.
                let inside = if phase.xcode_sub_steps > 0
                    && phase.message.to_ascii_lowercase().contains("xcode build")
                {
                    const TYPICAL_XCODE_STEPS: f64 = 120.0;
                    (phase.xcode_sub_steps as f64 / TYPICAL_XCODE_STEPS).min(0.95)
                } else {
                    let elapsed = phase.started_at.elapsed().as_secs_f64();
                    let estimated = phase_estimated_secs(phase) as f64;
                    (elapsed / estimated).min(0.95)
                };
                pct += weight * inside;
                break;
            }
        }
    }
    pct.min(99.0).round() as u8
}

/// Weight (in % points out of 100) of a single Flutter startup phase.
/// Calibrated against an iPhone build + first launch — Xcode is by far
/// the dominant cost, so it gets the lion's share of the bar. Sum of
/// all known weights ≈ 100; unknown phases get a default 8%.
fn phase_weight(phase: &crate::app::ProgressPhase) -> u8 {
    let m = phase.message.to_ascii_lowercase();
    if m.contains("xcode build") {
        return 65;
    }
    if m.contains("gradle") {
        return 60;
    }
    if m.contains("installing and launching") {
        return 20;
    }
    if m.contains("pod install") {
        return 8;
    }
    if m.contains("resolving dependencies") || m.contains("running pub get") {
        return 3;
    }
    if m.contains("waiting for connection") || m.contains("vm service") {
        return 4;
    }
    8
}

/// Typical duration of a phase, in seconds — used to interpolate the
/// bar smoothly between the opener and closer events. These are
/// rough-and-tuned numbers that match what the project README quotes
/// for an unoptimized iOS debug build (~90 s Xcode, ~15 s install).
/// Could become per-project learned values later, but a global default
/// is good enough for the bar to move at human speed.
fn phase_estimated_secs(phase: &crate::app::ProgressPhase) -> u64 {
    let m = phase.message.to_ascii_lowercase();
    if m.contains("xcode build") {
        return 90;
    }
    if m.contains("gradle") {
        return 45;
    }
    if m.contains("installing and launching") {
        return 15;
    }
    if m.contains("pod install") {
        return 30;
    }
    10
}

/// Performance + Devices panels in the lower portion of the viewport,
/// side-by-side on wide terminals and stacked on narrow ones.
fn render_status_panels(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    // The left panel swaps between Performance and Network based on
    // `state.show_network` (toggled with `n`). The right panel
    // (Devices) is always shown.
    let render_left = |rect: Rect, buf: &mut Buffer| {
        if state.show_network {
            panels::network::render_network(rect, buf, state, theme);
        } else {
            panels::performance::render_performance(rect, buf, state, theme);
        }
    };
    if area.width < NARROW_WIDTH {
        let cols = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        render_left(cols[0], buf);
        panels::devices::render_devices(cols[1], buf, state, theme);
    } else {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        render_left(cols[0], buf);
        panels::devices::render_devices(cols[1], buf, state, theme);
    }
}

fn render_too_small(area: Rect, buf: &mut Buffer, theme: &Theme) {
    use ratatui::layout::Alignment;
    let msg = format!(
        "Terminal too small ({}×{}). Need at least {MIN_WIDTH}×{MIN_HEIGHT}.",
        area.width, area.height
    );
    let line = Line::styled(msg, Style::default().fg(theme.warn).bg(theme.bg));
    Paragraph::new(line)
        .alignment(Alignment::Center)
        .render(area, buf);
}

fn render_header(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    let device = match state.active_sessions.len() {
        0 => "no device".to_string(),
        1 => state.active_sessions[0].display_name.clone(),
        n => format!("{n} devices"),
    };
    let elapsed = format_elapsed(state.elapsed());
    let chrono_icon = if state.compile_finished.is_some() {
        '✓'
    } else {
        '⏱'
    };
    let chrono_color = if state.compile_finished.is_some() {
        theme.success
    } else {
        theme.fg
    };
    let alpha = state.reload_flash_alpha();
    let bg = if alpha > 0.0 {
        lerp(theme.bg, theme.success, alpha * 0.4)
    } else {
        theme.bg
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent).bg(bg))
        .style(Style::default().fg(theme.fg).bg(bg));
    let inner = block.inner(area);
    block.render(area, buf);

    // Status label that shimmers while something is happening. Order
    // matters — most urgent / most-likely-to-block-the-user wins:
    //   1. Wi-Fi takeover preparing — a session is mid-attach, keys
    //      will refuse until ready
    //   2. Refresh — user just hit r/R
    //   3. Building — first compile not finished yet
    let takeover_in_progress = state
        .active_sessions
        .iter()
        .any(|s| matches!(s.state, fl_core::DeviceSessionState::Reloading));
    let status_text: Option<&'static str> = if takeover_in_progress {
        Some("📶 Wi-Fi…")
    } else if state.reload_flash_alpha() > 0.05 {
        Some("Refresh…")
    } else if state.compile_finished.is_none() {
        Some("Building…")
    } else {
        None
    };

    // Right segment: optional status + chrono, drawn together so the
    // shimmer sweep flows from the label straight through the digits.
    let chrono_text = format!("{chrono_icon} {elapsed}");
    let right_block = match status_text {
        Some(s) => format!("{s}  {chrono_text}"),
        None => chrono_text.clone(),
    };
    let right_width = right_block.chars().count() as u16 + 2; // 1-col padding each side
    let title_width = inner.width.saturating_sub(right_width);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(title_width),
            Constraint::Length(right_width),
        ])
        .split(inner);

    // Title is ALWAYS shown on the left — the banner doesn't replace
    // it, it overlays the center of the bar (see below). Keeps the
    // user oriented (which app / device / mode) even while a transient
    // status flashes by.
    let brightness_icon: &str = match state
        .brightness_state
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        crate::app::BRIGHTNESS_LIGHT => "☀️",
        crate::app::BRIGHTNESS_DARK => "🌙",
        _ => "⚙️",
    };
    let title_text = format!(
        " {brightness_icon}  flutter-cli ── {} · {} · {}",
        state.app_name, state.mode, device
    );
    let title = truncate_to_width(&title_text, cols[0].width as usize);
    Paragraph::new(Line::styled(
        title,
        Style::default()
            .fg(theme.accent)
            .bg(bg)
            .add_modifier(ratatui::style::Modifier::BOLD),
    ))
    .render(cols[0], buf);

    // Banner overlay (the "snackbar"). When set, paint it centered
    // horizontally in the header bar — overlapping the *middle* of
    // the title line so the user notices it but the left-aligned
    // title still shows on either side. Auto-expires after ~3 s
    // (see AppState::show_banner) and the bar returns to normal.
    if let Some(b) = &state.banner {
        let kind_color = match b.kind {
            BannerKind::Info => theme.cyan,
            BannerKind::Warn => theme.warn,
            BannerKind::Error => theme.error,
            BannerKind::Success => theme.success,
        };
        let label = format!(" {} ", b.message);
        let label_w = label.chars().count() as u16;
        // Only render if the bar is wide enough to fit the label
        // without crushing the title. If not, skip — better to drop
        // the snackbar than render an unreadable smush.
        if label_w + 4 <= inner.width {
            let x = inner.x + (inner.width.saturating_sub(label_w)) / 2;
            let overlay = Rect {
                x,
                y: inner.y,
                width: label_w,
                height: 1,
            };
            Paragraph::new(Line::styled(
                label,
                Style::default()
                    .fg(theme.bg)
                    .bg(kind_color)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ))
            .render(overlay, buf);
        }
    }

    // Right side: shimmer when something is happening, static otherwise.
    if status_text.is_some() {
        let phase = shimmer_phase(state.started_at.elapsed());
        let line = shimmer_line(&right_block, theme.dim, theme.accent, phase, bg);
        Paragraph::new(line)
            .alignment(ratatui::layout::Alignment::Right)
            .render(cols[1], buf);
    } else {
        Paragraph::new(Line::styled(
            format!("{chrono_text} "),
            Style::default().fg(chrono_color).bg(bg),
        ))
        .alignment(ratatui::layout::Alignment::Right)
        .render(cols[1], buf);
    }
}

/// Animation phase in `[0, 1)` cycling once every 1.5 s. Used by the
/// shimmer effect on the status label.
fn shimmer_phase(elapsed: std::time::Duration) -> f32 {
    let ms = elapsed.as_millis() as f32;
    (ms / 1500.0).fract()
}

/// Build a `Line` whose characters fade between `dim` and `accent` along a
/// sweep position determined by `phase`. The sweep moves left→right and
/// loops back to the start.
fn shimmer_line(
    text: &str,
    dim: ratatui::style::Color,
    accent: ratatui::style::Color,
    phase: f32,
    bg: ratatui::style::Color,
) -> Line<'static> {
    use ratatui::text::Span;
    let n = text.chars().count() as f32;
    let head = phase * (n + 6.0) - 3.0; // sweep slightly off-screen at both ends
    let spans: Vec<Span<'static>> = text
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let dist = (head - i as f32).abs();
            let t = (1.0 - (dist / 3.5)).clamp(0.0, 1.0);
            let color = lerp(dim, accent, t);
            Span::styled(c.to_string(), Style::default().fg(color).bg(bg))
        })
        .collect();
    Line::from(spans)
}

fn format_elapsed(d: std::time::Duration) -> String {
    let total = d.as_secs();
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Build the footer line for the available width by walking
/// `FOOTER_BINDS` in priority order and stopping when the next
/// entry would overflow. Returns an empty string when even the
/// first entry doesn't fit (caller falls back to `FOOTER_SHORT`).
fn pack_footer_binds(width: usize) -> String {
    let mut out = String::from(" ");
    // The trailing space mirrors the leading one so the line looks
    // visually centred when there's slack on the right.
    let trailing = 1;
    for (i, bind) in FOOTER_BINDS.iter().enumerate() {
        // Two spaces between entries, matching the previous tiers.
        let extra = if i == 0 {
            bind.chars().count()
        } else {
            2 + bind.chars().count()
        };
        if out.chars().count() + extra + trailing > width {
            break;
        }
        if i > 0 {
            out.push_str("  ");
        }
        out.push_str(bind);
    }
    if out.chars().count() == 1 {
        // Nothing fit — let the caller use FOOTER_SHORT.
        return String::new();
    }
    out.push(' ');
    out
}

fn render_footer(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    // Post-VM-Service: pack as many keybinds as the width allows,
    // dropping low-priority ones first. Falls back to a compact
    // single-line summary on very narrow terminals.
    if state.app_ready() {
        let line = pack_footer_binds(area.width as usize);
        let chosen = if line.is_empty() {
            FOOTER_SHORT.to_string()
        } else {
            line
        };
        Paragraph::new(Line::styled(chosen, theme.dimmed())).render(area, buf);
        return;
    }

    // Pre-ready: render a real percentage bar (driven by daemon
    // progress events) in place of the old shimmer hint. The static
    // keybinds suffix is kept dimmed and right-aligned. Format on a
    // wide terminal:
    //
    //   ` ⏳ 47% ████████████░░░░░░░░░░░░░░░  [e] error ↗  [/] filter  [c] copy 📋  [q] quit `
    //
    // The bar shrinks (and the static suffix degrades) on narrower
    // terminals; on the very narrowest we drop the keybinds entirely
    // so the bar always survives.
    let pct = estimated_percentage(state);

    let (label, static_text) = if area.width as usize >= 90 {
        (format!(" ⏳ {pct:>2}% "), FOOTER_FULL_PRE_READY_STATIC)
    } else if area.width as usize >= 60 {
        (format!(" ⏳ {pct:>2}% "), FOOTER_MEDIUM_PRE_READY_STATIC)
    } else {
        (format!(" ⏳ {pct:>2}% "), FOOTER_SHORT_PRE_READY_STATIC)
    };

    let label_w = label.chars().count();
    let static_w = static_text.chars().count();
    // Bar takes whatever's left between the label and the static suffix.
    // 1-col cushion on each side keeps it from kissing the borders.
    let bar_budget = (area.width as usize)
        .saturating_sub(label_w)
        .saturating_sub(static_w)
        .saturating_sub(2);
    // Clamp the bar to a comfortable maximum so super-wide terminals
    // don't render a 200-cell-long bar that's visually noisy.
    let bar_w = bar_budget.min(60);
    let bar = build_bar(bar_w, pct);

    let bg = theme.bg;
    let bar_style = if pct >= 99 {
        Style::default().fg(theme.success).bg(bg)
    } else {
        Style::default().fg(theme.accent).bg(bg)
    };
    let mid_pad = " ".repeat(bar_budget.saturating_sub(bar_w).max(1));

    use ratatui::text::Span;
    let spans = vec![
        Span::styled(label, Style::default().fg(theme.fg).bg(bg)),
        Span::styled(bar, bar_style),
        Span::styled(mid_pad, Style::default().bg(bg)),
        Span::styled(static_text.to_string(), theme.dimmed()),
    ];
    Paragraph::new(Line::from(spans)).render(area, buf);
}

fn truncate_to_width(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    s.chars().take(max_chars - 1).collect::<String>() + "…"
}

fn lerp(a: ratatui::style::Color, b: ratatui::style::Color, t: f32) -> ratatui::style::Color {
    use ratatui::style::Color;
    let (ar, ag, ab) = match a {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    };
    let (br, bg, bb) = match b {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    };
    let mix = |x: u8, y: u8| {
        ((x as f32) + ((y as f32) - (x as f32)) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_does_not_panic_on_small_area() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        let state = AppState::new("my_app".into(), "debug".into());
        render(
            Rect::new(0, 0, 80, 24),
            &mut buf,
            &state,
            &Theme::TOKYO_NIGHT,
        );
        let header_cell = &buf[(1, 1)];
        let _ = header_cell.symbol().to_owned();
    }

    fn dump(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn very_small_terminal_shows_too_small_message() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 8));
        let state = AppState::new("my_app".into(), "debug".into());
        render(
            Rect::new(0, 0, 30, 8),
            &mut buf,
            &state,
            &Theme::TOKYO_NIGHT,
        );
        let text = dump(&buf);
        assert!(
            text.contains("too small"),
            "missing too-small message, got:\n{text}"
        );
    }

    #[test]
    fn narrow_terminal_uses_vertical_stack() {
        // 70-wide is below NARROW_WIDTH (90) → Performance/Devices stack
        // vertically. Logs are NOT in the inline viewport any more (they
        // flow into the terminal's scrollback via print_above_viewport),
        // so the only two panels we expect to find are these two.
        let mut buf = Buffer::empty(Rect::new(0, 0, 70, 30));
        let state = AppState::new("my_app".into(), "debug".into());
        render(
            Rect::new(0, 0, 70, 30),
            &mut buf,
            &state,
            &Theme::TOKYO_NIGHT,
        );
        let text = dump(&buf);
        assert!(text.contains("Performance"), "missing Performance panel");
        assert!(text.contains("Devices"), "missing Devices panel");
    }

    #[test]
    fn footer_pre_ready_shows_progress_bar_and_hides_extension_keys() {
        // A fresh AppState has vm_connected == false → footer should
        // render the progress bar variant and OMIT r/R/b/p/P/o keys
        // (they're no-ops until the VM Service is up).
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 20));
        let state = AppState::new("my_app".into(), "debug".into());
        render(
            Rect::new(0, 0, 60, 20),
            &mut buf,
            &state,
            &Theme::TOKYO_NIGHT,
        );
        let text = dump(&buf);
        // Bar surface: there must be a `%` token AND at least one bar
        // glyph somewhere on the footer row.
        assert!(text.contains('%'), "missing % in footer:\n{text}");
        assert!(
            text.chars().any(|c| c == '█' || c == '░'),
            "missing bar glyphs:\n{text}"
        );
        assert!(
            !text.contains("r reload") && !text.contains("[r] reload"),
            "pre-ready footer should NOT advertise reload:\n{text}"
        );
    }

    #[test]
    fn footer_shows_full_keys_once_vm_service_is_ready() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 120, 24));
        let mut state = AppState::new("my_app".into(), "debug".into());
        state.vm_connected = true;
        render(
            Rect::new(0, 0, 120, 24),
            &mut buf,
            &state,
            &Theme::TOKYO_NIGHT,
        );
        let text = dump(&buf);
        assert!(
            text.contains("[r] reload"),
            "post-ready footer should advertise reload:\n{text}"
        );
    }

    #[test]
    fn truncate_to_width_keeps_short_strings_intact() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
        assert_eq!(truncate_to_width("hello", 5), "hello");
    }

    #[test]
    fn truncate_to_width_ellipsizes_long_strings() {
        assert_eq!(truncate_to_width("hello world", 8), "hello w…");
        assert_eq!(truncate_to_width("hello", 1), "…");
        assert_eq!(truncate_to_width("hello", 0), "");
    }

    #[test]
    fn format_elapsed_under_one_hour_is_mmss() {
        use std::time::Duration;
        assert_eq!(format_elapsed(Duration::from_secs(0)), "00:00");
        assert_eq!(format_elapsed(Duration::from_secs(83)), "01:23");
        assert_eq!(format_elapsed(Duration::from_secs(3599)), "59:59");
    }

    #[test]
    fn format_elapsed_over_one_hour_is_hhmmss() {
        use std::time::Duration;
        assert_eq!(format_elapsed(Duration::from_secs(3600)), "01:00:00");
        assert_eq!(format_elapsed(Duration::from_secs(7384)), "02:03:04");
    }

    #[test]
    fn header_includes_chrono_with_running_icon() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 24));
        let state = AppState::new("my_app".into(), "debug".into());
        render(
            Rect::new(0, 0, 100, 24),
            &mut buf,
            &state,
            &Theme::TOKYO_NIGHT,
        );
        let text = dump(&buf);
        assert!(text.contains('⏱'), "missing chrono icon, got:\n{text}");
        assert!(text.contains("00:00"), "missing elapsed time, got:\n{text}");
    }

    #[test]
    fn header_chrono_switches_to_checkmark_after_compile_finishes() {
        // The chronometer flips from ⏱ to ✓ only when EVERY active
        // session has reached `Ready`. Register a single session
        // and flip it Ready — that's enough for the green checkmark.
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 24));
        let mut state = AppState::new("my_app".into(), "debug".into());
        state.apply(fl_core::AppEvent::Device(
            fl_core::DeviceEvent::SessionState {
                serial: "ABC".into(),
                state: fl_core::DeviceSessionState::Ready,
            },
        ));
        render(
            Rect::new(0, 0, 100, 24),
            &mut buf,
            &state,
            &Theme::TOKYO_NIGHT,
        );
        let text = dump(&buf);
        assert!(
            text.contains('✓'),
            "expected checkmark after Ready, got:\n{text}"
        );
        assert!(
            !text.contains('⏱'),
            "chrono running icon should be gone, got:\n{text}"
        );
    }

    #[test]
    fn header_chrono_stays_running_while_a_second_device_is_still_building() {
        // Real multi-device flow: both sessions are registered
        // (Connecting) at startup, then the fastest one flips to
        // Ready. The chronometer must NOT flip to ✓ yet — there's
        // still a second app compiling.
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 24));
        let mut state = AppState::new("my_app".into(), "debug".into());
        state.apply(fl_core::AppEvent::Device(
            fl_core::DeviceEvent::SessionState {
                serial: "ABC".into(),
                state: fl_core::DeviceSessionState::Connecting,
            },
        ));
        state.apply(fl_core::AppEvent::Device(
            fl_core::DeviceEvent::SessionState {
                serial: "XYZ".into(),
                state: fl_core::DeviceSessionState::Connecting,
            },
        ));
        // First device finishes building.
        state.apply(fl_core::AppEvent::Device(
            fl_core::DeviceEvent::SessionState {
                serial: "ABC".into(),
                state: fl_core::DeviceSessionState::Ready,
            },
        ));
        render(
            Rect::new(0, 0, 100, 24),
            &mut buf,
            &state,
            &Theme::TOKYO_NIGHT,
        );
        let text = dump(&buf);
        assert!(
            text.contains('⏱'),
            "chrono should still be running, got:\n{text}"
        );
        assert!(
            !text.contains('✓'),
            "no checkmark while a device builds, got:\n{text}"
        );
    }

    // ── progress bar helpers ─────────────────────────────────────────────

    fn make_phase(msg: &str) -> crate::app::ProgressPhase {
        crate::app::ProgressPhase {
            id: "1".into(),
            progress_id: None,
            message: msg.into(),
            started_at: std::time::Instant::now(),
            finished_at: None,
            xcode_sub_steps: 0,
        }
    }

    // ── build_bar ────────────────────────────────────────────────────────

    #[test]
    fn build_bar_width_is_constant_and_fill_is_proportional() {
        // Empty bar
        let b0 = build_bar(20, 0);
        assert_eq!(b0.chars().count(), 20);
        assert_eq!(b0.chars().filter(|c| *c == '█').count(), 0);
        // Half-full bar
        let b50 = build_bar(20, 50);
        assert_eq!(b50.chars().count(), 20);
        assert_eq!(b50.chars().filter(|c| *c == '█').count(), 10);
        // Full bar
        let b100 = build_bar(20, 100);
        assert_eq!(b100.chars().count(), 20);
        assert_eq!(b100.chars().filter(|c| *c == '█').count(), 20);
    }

    #[test]
    fn build_bar_caps_pct_above_100() {
        // pct values >100 shouldn't overflow the fill count.
        let b = build_bar(10, 250);
        assert_eq!(b.chars().filter(|c| *c == '█').count(), 10);
    }

    // ── phase weights / estimated_percentage ─────────────────────────────

    #[test]
    fn phase_weight_xcode_build_dominates() {
        // Xcode is the longest phase by far → it carries the lion's share.
        let w_xcode = phase_weight(&make_phase("Running Xcode build..."));
        let w_install = phase_weight(&make_phase("Installing and launching..."));
        let w_pods = phase_weight(&make_phase("Running pod install..."));
        assert!(w_xcode > w_install);
        assert!(w_install > w_pods);
        assert_eq!(w_xcode, 65);
    }

    #[test]
    fn estimated_percentage_is_100_when_ready() {
        let mut state = AppState::new("a".into(), "d".into());
        state.vm_connected = true;
        assert_eq!(estimated_percentage(&state), 100);
    }

    #[test]
    fn estimated_percentage_is_capped_at_99_pre_ready() {
        // Simulate a completed Xcode build (65%) + completed install
        // (20%) + completed VM service (4%) — sum is 89%. Bar still
        // shouldn't show 100% until vm_connected.
        let mut state = AppState::new("a".into(), "d".into());
        let now = std::time::Instant::now();
        state.progress_phases.push(crate::app::ProgressPhase {
            id: "1".into(),
            progress_id: None,
            message: "Running Xcode build...".into(),
            started_at: now,
            finished_at: Some(now),
            xcode_sub_steps: 0,
        });
        state.progress_phases.push(crate::app::ProgressPhase {
            id: "2".into(),
            progress_id: None,
            message: "Installing and launching...".into(),
            started_at: now,
            finished_at: Some(now),
            xcode_sub_steps: 0,
        });
        let pct = estimated_percentage(&state);
        assert!((85..=99).contains(&pct), "got pct={pct}");
    }

    #[test]
    fn estimated_percentage_seed_value_when_no_phases_yet() {
        // First few seconds of activity: bar shouldn't be at 0 (looks
        // frozen) but shouldn't fabricate either. Capped at 3.
        let state = AppState::new("a".into(), "d".into());
        let pct = estimated_percentage(&state);
        assert!(pct <= 3);
    }

    #[test]
    fn xcode_sub_steps_drive_real_progress_when_available() {
        use fl_core::{AppEvent, FlutterEvent};
        let mut state = AppState::new("a".into(), "d".into());
        // Parent phase
        state.apply(AppEvent::Flutter(FlutterEvent::Progress {
            id: "1".into(),
            progress_id: None,
            message: "Running Xcode build...".into(),
            finished: false,
        }));
        let p0 = estimated_percentage(&state);

        // Feed 60 xcode.build.line sub-events under the parent (Xcode is 65%
        // weight, denominator 120, so 60/120 → 50% inside-phase →
        // ~32% pct).
        for _ in 0..60 {
            state.apply(AppEvent::Flutter(FlutterEvent::Progress {
                id: "sub".into(),
                progress_id: Some("xcode.build.line".into()),
                message: "Compile main.m".into(),
                finished: false,
            }));
        }
        let p_after = estimated_percentage(&state);
        assert!(
            p_after > p0,
            "sub-steps should advance the bar: before={p0} after={p_after}"
        );
        assert!(
            (25..=40).contains(&p_after),
            "60/120 of a 65% phase ≈ 32%, got {p_after}"
        );
    }

    #[test]
    fn xcode_sub_step_events_dont_create_new_phases() {
        use fl_core::{AppEvent, FlutterEvent};
        let mut state = AppState::new("a".into(), "d".into());
        state.apply(AppEvent::Flutter(FlutterEvent::Progress {
            id: "1".into(),
            progress_id: None,
            message: "Running Xcode build...".into(),
            finished: false,
        }));
        for _ in 0..5 {
            state.apply(AppEvent::Flutter(FlutterEvent::Progress {
                id: "sub".into(),
                progress_id: Some("xcode.build.line".into()),
                message: "CompileSwift Foo.swift".into(),
                finished: false,
            }));
        }
        assert_eq!(state.progress_phases.len(), 1);
        assert_eq!(state.progress_phases[0].xcode_sub_steps, 5);
    }

    #[test]
    fn footer_pre_ready_renders_percent_and_bar_glyphs() {
        // No phase yet → tiny seed %. Bar should still be present.
        use fl_core::{AppEvent, FlutterEvent};
        let mut state = AppState::new("a".into(), "d".into());
        state.apply(AppEvent::Flutter(FlutterEvent::Progress {
            id: "1".into(),
            progress_id: None,
            message: "Running Xcode build...".into(),
            finished: false,
        }));
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 24));
        render(
            Rect::new(0, 0, 100, 24),
            &mut buf,
            &state,
            &Theme::TOKYO_NIGHT,
        );
        let text = dump_buffer(&buf);
        assert!(text.contains('%'), "footer should print %, got:\n{text}");
        assert!(
            text.chars().any(|c| c == '█' || c == '░'),
            "bar glyphs missing:\n{text}"
        );
    }

    #[test]
    fn progress_phase_is_marked_finished_on_close_event() {
        use fl_core::{AppEvent, FlutterEvent};
        let mut state = AppState::new("a".into(), "d".into());
        state.apply(AppEvent::Flutter(FlutterEvent::Progress {
            id: "7".into(),
            progress_id: None,
            message: "Running Xcode build...".into(),
            finished: false,
        }));
        assert!(state.current_progress_phase().is_some());
        state.apply(AppEvent::Flutter(FlutterEvent::Progress {
            id: "7".into(),
            progress_id: None,
            message: "".into(),
            finished: true,
        }));
        assert!(
            state.current_progress_phase().is_none(),
            "closer event flips finished_at"
        );
        assert_eq!(state.progress_phases.len(), 1);
        assert!(state.progress_phases[0].finished_at.is_some());
    }

    #[test]
    fn dashboard_snapshot() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 24));
        let mut state = AppState::new("my_app".into(), "debug".into());
        state.apply(fl_core::AppEvent::Flutter(fl_core::FlutterEvent::Log {
            level: fl_core::LogLevel::Info,
            message: "App started".into(),
        }));
        render(
            Rect::new(0, 0, 100, 24),
            &mut buf,
            &state,
            &Theme::TOKYO_NIGHT,
        );
        let dump = dump_buffer(&buf);
        insta::assert_snapshot!(dump);
    }

    fn dump_buffer(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }
}
