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

// Pre-ready variants: shown while the app is still compiling /
// installing / waiting on VM Service. We omit r/R/b/p/P/o because
// they're no-ops at that point — the user pressing them just spams
// the log with "not ready" warnings.
//
// `e` (jump to error in IDE) IS surfaced here even though the app
// isn't running: it's the moment the user needs it most, since most
// pre-ready failures are Dart compilation errors with a file ref in
// the log. Same reason `/` (filter) and `c` (copy) stay.
//
// The prefix (the "building" hint) shimmers; the suffix (the static
// keys) is rendered with the normal dimmed footer style. Splitting
// avoids running the shimmer animation over key labels, which would
// look messy.
const FOOTER_FULL_PRE_READY_SHIMMER: &str = " ⏳ building app… ";
const FOOTER_FULL_PRE_READY_STATIC: &str =
    " [e] error ↗  [/] filter  [c] copy 📋  [q] quit ";
const FOOTER_MEDIUM_PRE_READY_SHIMMER: &str = " ⏳ building… ";
const FOOTER_MEDIUM_PRE_READY_STATIC: &str = " [e] err  [q] quit ";
const FOOTER_SHORT_PRE_READY_SHIMMER: &str = " ⏳ building ";
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
    // Inline-viewport layout (Claude-Code style). Logs no longer live
    // in this buffer — they're printed directly into the terminal's
    // scrollback (see `TuiRunner::print_above_viewport`) and scroll
    // naturally above the box. What remains here, pinned to the bottom
    // of the terminal, is the live status surface:
    //   1. fl-info status header   (3 rows)
    //   2. Loading progress strip  (1 row — pre-ready only, hidden when
    //                               VM Service is connected)
    //   3. Performance + Devices   (flex — takes the remaining rows)
    //   4. Footer keybinds         (1 row)
    let show_progress = !state.app_ready() && !state.progress_phases.is_empty();
    let constraints: &[Constraint] = if show_progress {
        &[
            Constraint::Length(HEADER_HEIGHT),
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(1),
        ]
    } else {
        &[
            Constraint::Length(HEADER_HEIGHT),
            Constraint::Min(4),
            Constraint::Length(1),
        ]
    };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    render_header(layout[0], buf, state, theme);
    if show_progress {
        render_progress_strip(layout[1], buf, state, theme);
        render_status_panels(layout[2], buf, state, theme);
        render_footer(layout[3], buf, state, theme);
    } else {
        render_status_panels(layout[1], buf, state, theme);
        render_footer(layout[2], buf, state, theme);
    }
}

/// Pre-ready loading strip: stepper of completed phases + the one
/// currently active, plus an indeterminate animated bar that conveys
/// "yes, still working" without faking a percentage. Renders into
/// a single row right below the header.
///
/// Format on a wide terminal:
///
///   `[████░░░░░] ⏳ Installing and launching... · 0:08   ✓ Resolve [0.4s] · ✓ Build [4:18] · ⏳ Install [0:08]`
///
/// On narrow terminals the right-side stepper is dropped — current
/// phase + indeterminate bar always survive.
fn render_progress_strip(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    use ratatui::text::Span;
    if area.width == 0 {
        return;
    }
    let current = state.current_progress_phase();
    let phase_label = current
        .map(|p| p.message.as_str())
        .unwrap_or("Waiting for next phase…");
    let phase_elapsed = current.map(|p| p.started_at.elapsed()).unwrap_or_default();
    let label = format!(" ⏳ {phase_label} · {} ", format_short(phase_elapsed));

    // Indeterminate bar — fixed width, animated sweep.
    const BAR_W: usize = 14;
    let bar = indeterminate_bar(BAR_W, state.started_at.elapsed());

    // Right side: compact phase stepper. We pack as many phases as fit
    // into whatever width is left after the bar + label.
    let stepper = render_phase_stepper(state);

    let label_w = label.chars().count();
    let bar_w = BAR_W + 2; // `[` + bar + `]`
    let stepper_w = stepper.chars().count();
    let total_w = label_w + bar_w + 1 + stepper_w; // 1 = gap
    let bg = theme.bg;
    let dim = theme.dim;
    let accent = theme.accent;

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(4);
    spans.push(Span::styled(
        format!("[{bar}]"),
        Style::default().fg(accent).bg(bg),
    ));
    spans.push(Span::styled(label, Style::default().fg(theme.fg).bg(bg)));
    if total_w <= area.width as usize {
        spans.push(Span::styled(
            " ".repeat(area.width as usize - label_w - bar_w - stepper_w),
            Style::default().bg(bg),
        ));
        spans.push(Span::styled(stepper, Style::default().fg(dim).bg(bg)));
    } else {
        // Drop the stepper, pad with bg so we don't leave glyphs behind.
        let pad = (area.width as usize).saturating_sub(label_w + bar_w);
        spans.push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
    }
    Paragraph::new(Line::from(spans)).render(area, buf);
}

/// A `BAR_W`-wide indeterminate bar with a 3-cell highlight that
/// sweeps left-to-right and bounces off the edges. Uses `█` for the
/// highlight and `░` for the rest.
fn indeterminate_bar(width: usize, elapsed: std::time::Duration) -> String {
    if width == 0 {
        return String::new();
    }
    let cycle_ms = 1400u128;
    let t = (elapsed.as_millis() % cycle_ms) as f32 / cycle_ms as f32;
    // Triangular wave: 0 → 1 → 0
    let phase = if t < 0.5 { t * 2.0 } else { (1.0 - t) * 2.0 };
    let head_w = 3usize;
    let head_pos = (phase * (width.saturating_sub(head_w) as f32)).round() as usize;
    let mut out = String::with_capacity(width);
    for i in 0..width {
        if i >= head_pos && i < head_pos + head_w {
            out.push('█');
        } else {
            out.push('░');
        }
    }
    out
}

/// Format a `Duration` in a compact, terminal-friendly way:
/// `< 60s` → `12s`, `< 1h` → `4:18`, otherwise `1:02:31`.
fn format_short(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{m}:{s:02}", m = secs / 60, s = secs % 60)
    } else {
        format!(
            "{h}:{m:02}:{s:02}",
            h = secs / 3600,
            m = (secs % 3600) / 60,
            s = secs % 60
        )
    }
}

/// Build the compact phase stepper string from `state.progress_phases`.
/// Done phases get `✓ Title [elapsed]`, the active phase gets
/// `⏳ Title [elapsed]`, separated by ` · `.
fn render_phase_stepper(state: &AppState) -> String {
    if state.progress_phases.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::with_capacity(state.progress_phases.len());
    for phase in &state.progress_phases {
        let title = friendly_phase_title(phase);
        let elapsed = match phase.finished_at {
            Some(end) => end.duration_since(phase.started_at),
            None => phase.started_at.elapsed(),
        };
        let mark = if phase.finished_at.is_some() {
            "✓"
        } else {
            "⏳"
        };
        parts.push(format!("{mark} {title} [{}]", format_short(elapsed)));
    }
    parts.join(" · ")
}

/// Strip trailing ellipses and shorten the most common Flutter
/// phase messages to a few-letter title — keeps the stepper readable
/// at every terminal width.
fn friendly_phase_title(phase: &crate::app::ProgressPhase) -> String {
    let m = phase.message.trim_end_matches('…').trim_end_matches('.').trim();
    // Match against known startup phases first — saves columns and gives
    // the user a recognisable label.
    let lower = m.to_ascii_lowercase();
    if lower.contains("xcode build") {
        return "Build".to_string();
    }
    if lower.contains("installing and launching") {
        return "Install".to_string();
    }
    if lower.contains("pod install") {
        return "Pods".to_string();
    }
    if lower.contains("resolving dependencies") || lower.contains("running pub get") {
        return "Resolve".to_string();
    }
    if lower.starts_with("performing hot reload") {
        return "Hot reload".to_string();
    }
    if lower.starts_with("performing hot restart") {
        return "Hot restart".to_string();
    }
    if lower.starts_with("compiling ") {
        return "Compile".to_string();
    }
    if lower.contains("connecting to the vm service")
        || lower.contains("waiting for connection from debug service")
    {
        return "VM".to_string();
    }
    // Fallback: keep the first 18 visible chars so the stepper stays
    // narrow. The full message lives in scrollback for the curious.
    m.chars().take(18).collect()
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

    // Pre-ready: shimmer the "building app…" hint to signal "work in
    // progress, please wait" — the same animation we use for the
    // header "Building…" status so the two feel unified.
    let (shimmer_text, static_text) = if area.width as usize
        >= (FOOTER_FULL_PRE_READY_SHIMMER.chars().count()
            + FOOTER_FULL_PRE_READY_STATIC.chars().count())
    {
        (FOOTER_FULL_PRE_READY_SHIMMER, FOOTER_FULL_PRE_READY_STATIC)
    } else if area.width as usize
        >= (FOOTER_MEDIUM_PRE_READY_SHIMMER.chars().count()
            + FOOTER_MEDIUM_PRE_READY_STATIC.chars().count())
    {
        (
            FOOTER_MEDIUM_PRE_READY_SHIMMER,
            FOOTER_MEDIUM_PRE_READY_STATIC,
        )
    } else {
        (
            FOOTER_SHORT_PRE_READY_SHIMMER,
            FOOTER_SHORT_PRE_READY_STATIC,
        )
    };

    let phase = shimmer_phase(state.started_at.elapsed());
    let shimmer = shimmer_line(shimmer_text, theme.dim, theme.accent, phase, theme.bg);
    let mut spans = shimmer.spans;
    spans.push(ratatui::text::Span::styled(
        static_text.to_string(),
        theme.dimmed(),
    ));
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
    fn footer_pre_ready_hides_extension_keys_and_shows_building_hint() {
        // A fresh AppState has vm_connected == false → footer should
        // be the pre-ready variant and OMIT r/R/b/p/P/o entirely.
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 20));
        let state = AppState::new("my_app".into(), "debug".into());
        render(
            Rect::new(0, 0, 60, 20),
            &mut buf,
            &state,
            &Theme::TOKYO_NIGHT,
        );
        let text = dump(&buf);
        assert!(text.contains("building"), "missing building hint:\n{text}");
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

    // ── progress strip helpers ───────────────────────────────────────────

    #[test]
    fn format_short_under_a_minute() {
        use std::time::Duration;
        assert_eq!(format_short(Duration::from_secs(0)), "0s");
        assert_eq!(format_short(Duration::from_secs(8)), "8s");
        assert_eq!(format_short(Duration::from_secs(59)), "59s");
    }

    #[test]
    fn format_short_minutes_then_hours() {
        use std::time::Duration;
        assert_eq!(format_short(Duration::from_secs(60)), "1:00");
        assert_eq!(format_short(Duration::from_secs(4 * 60 + 18)), "4:18");
        assert_eq!(format_short(Duration::from_secs(3600)), "1:00:00");
        assert_eq!(format_short(Duration::from_secs(3661)), "1:01:01");
    }

    #[test]
    fn indeterminate_bar_has_three_full_blocks() {
        use std::time::Duration;
        let s = indeterminate_bar(14, Duration::from_millis(0));
        assert_eq!(s.chars().count(), 14);
        let full = s.chars().filter(|c| *c == '█').count();
        let dim = s.chars().filter(|c| *c == '░').count();
        assert_eq!(full, 3, "exactly 3 head cells highlighted");
        assert_eq!(dim, 11);
    }

    #[test]
    fn indeterminate_bar_head_moves_with_time() {
        use std::time::Duration;
        let a = indeterminate_bar(14, Duration::from_millis(0));
        let b = indeterminate_bar(14, Duration::from_millis(200));
        assert_ne!(a, b, "the head should have moved");
    }

    fn make_phase(msg: &str) -> crate::app::ProgressPhase {
        crate::app::ProgressPhase {
            id: "1".into(),
            progress_id: None,
            message: msg.into(),
            started_at: std::time::Instant::now(),
            finished_at: None,
        }
    }

    #[test]
    fn friendly_phase_title_known_messages_get_short_labels() {
        assert_eq!(
            friendly_phase_title(&make_phase("Running Xcode build...")),
            "Build"
        );
        assert_eq!(
            friendly_phase_title(&make_phase("Installing and launching...")),
            "Install"
        );
        assert_eq!(
            friendly_phase_title(&make_phase("Running pod install...")),
            "Pods"
        );
        assert_eq!(
            friendly_phase_title(&make_phase("Resolving dependencies in `my_app`...")),
            "Resolve"
        );
        assert_eq!(
            friendly_phase_title(&make_phase("Performing hot reload...")),
            "Hot reload"
        );
    }

    #[test]
    fn friendly_phase_title_unknown_message_is_truncated() {
        let p = make_phase("Some quite long phase message that wouldn't fit");
        let t = friendly_phase_title(&p);
        assert!(t.chars().count() <= 18, "got: {t}");
    }

    #[test]
    fn progress_strip_only_visible_when_pre_ready_and_has_phases() {
        use fl_core::{AppEvent, FlutterEvent};
        let mut state = AppState::new("a".into(), "d".into());
        // No phases yet → strip stays hidden, dashboard layout unchanged.
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 24));
        render(Rect::new(0, 0, 100, 24), &mut buf, &state, &Theme::TOKYO_NIGHT);
        let text_before = dump_buffer(&buf);
        assert!(
            !text_before.contains("⏳ Running Xcode build"),
            "no phase yet → strip hidden"
        );
        // Apply a Progress event with non-empty message → phase active.
        state.apply(AppEvent::Flutter(FlutterEvent::Progress {
            id: "1".into(),
            progress_id: None,
            message: "Running Xcode build...".into(),
            finished: false,
        }));
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 24));
        render(Rect::new(0, 0, 100, 24), &mut buf, &state, &Theme::TOKYO_NIGHT);
        let text_after = dump_buffer(&buf);
        assert!(
            text_after.contains("Running Xcode build"),
            "active phase → strip visible:\n{text_after}"
        );
        assert!(
            text_after.contains("Build"),
            "stepper short-label visible:\n{text_after}"
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
