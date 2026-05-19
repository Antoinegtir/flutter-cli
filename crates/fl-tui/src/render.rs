//! Top-level dashboard render: header + body split + footer + optional banner.

use crate::app::{AppState, BannerKind};
use crate::panels;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

const FOOTER_FULL: &str = " [r] reload  [R] restart  [b] theme  [p] paint  [P] perf  [o] platform  [s] snap 📸  [n] net  [d] devtools  [/] filter  [c] copy 📋  [q] quit ";
const FOOTER_MEDIUM: &str = " [r] reload  [R] restart  [b] theme  [q] quit ";
const FOOTER_SHORT: &str = " r reload · q quit ";

// Pre-ready variants: shown while the app is still compiling /
// installing / waiting on VM Service. We omit r/R/b/p/P/o because
// they're no-ops at that point — the user pressing them just spams
// the log with "not ready" warnings.
//
// The prefix (the "building" hint) shimmers; the suffix (the static
// keys) is rendered with the normal dimmed footer style. Splitting
// avoids running the shimmer animation over key labels, which would
// look messy.
const FOOTER_FULL_PRE_READY_SHIMMER: &str = " ⏳ building app… ";
const FOOTER_FULL_PRE_READY_STATIC: &str = " [/] filter  [c] copy 📋  [q] quit ";
const FOOTER_MEDIUM_PRE_READY_SHIMMER: &str = " ⏳ building… ";
const FOOTER_MEDIUM_PRE_READY_STATIC: &str = " [q] quit ";
const FOOTER_SHORT_PRE_READY_SHIMMER: &str = " ⏳ building ";
const FOOTER_SHORT_PRE_READY_STATIC: &str = "· q quit ";

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
    // naturally above the box. The ASCII banner is also printed once
    // at init into scrollback. What remains here, pinned to the
    // bottom of the terminal, is the live status surface:
    //   1. fl-info status header   (3 rows: app · mode · device + chrono;
    //                              banners take over the title slot)
    //   2. Performance + Devices   (flex — takes the remaining rows)
    //   3. Footer keybinds         (1 row)
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
    let chrono_icon = if state.compile_finished.is_some() { '✓' } else { '⏱' };
    let chrono_color = if state.compile_finished.is_some() { theme.success } else { theme.fg };
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
        .constraints([Constraint::Length(title_width), Constraint::Length(right_width)])
        .split(inner);

    // Title is ALWAYS shown on the left — the banner doesn't replace
    // it, it overlays the center of the bar (see below). Keeps the
    // user oriented (which app / device / mode) even while a transient
    // status flashes by.
    let brightness_icon: &str =
        match state.brightness_state.load(std::sync::atomic::Ordering::Relaxed) {
            crate::app::BRIGHTNESS_LIGHT => "☀️",
            crate::app::BRIGHTNESS_DARK => "🌙",
            _ => "⚙️",
        };
    let title_text = format!(
        " {brightness_icon}  fl ── {} · {} · {}",
        state.app_name, state.mode, device
    );
    let title = truncate_to_width(&title_text, cols[0].width as usize);
    Paragraph::new(Line::styled(
        title,
        Style::default().fg(theme.accent).bg(bg)
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
            BannerKind::Info    => theme.cyan,
            BannerKind::Warn    => theme.warn,
            BannerKind::Error   => theme.error,
            BannerKind::Success => theme.success,
        };
        let label = format!(" {} ", b.message);
        let label_w = label.chars().count() as u16;
        // Only render if the bar is wide enough to fit the label
        // without crushing the title. If not, skip — better to drop
        // the snackbar than render an unreadable smush.
        if label_w + 4 <= inner.width {
            let x = inner.x + (inner.width.saturating_sub(label_w)) / 2;
            let overlay = Rect { x, y: inner.y, width: label_w, height: 1 };
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

fn render_footer(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    // Post-VM-Service: a flat dimmed line with all the action keys.
    if state.app_ready() {
        let chosen = if area.width as usize >= FOOTER_FULL.chars().count() {
            FOOTER_FULL
        } else if area.width as usize >= FOOTER_MEDIUM.chars().count() {
            FOOTER_MEDIUM
        } else {
            FOOTER_SHORT
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
        (FOOTER_MEDIUM_PRE_READY_SHIMMER, FOOTER_MEDIUM_PRE_READY_STATIC)
    } else {
        (FOOTER_SHORT_PRE_READY_SHIMMER, FOOTER_SHORT_PRE_READY_STATIC)
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
    let (ar, ag, ab) = match a { Color::Rgb(r,g,b)=>(r,g,b), _=>(0,0,0) };
    let (br, bg, bb) = match b { Color::Rgb(r,g,b)=>(r,g,b), _=>(0,0,0) };
    let mix = |x: u8, y: u8| ((x as f32) + ((y as f32) - (x as f32)) * t).round().clamp(0.0, 255.0) as u8;
    Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_does_not_panic_on_small_area() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        let state = AppState::new("my_app".into(), "debug".into());
        render(Rect::new(0, 0, 80, 24), &mut buf, &state, &Theme::TOKYO_NIGHT);
        let header_cell = buf.get(1, 1);
        let _ = header_cell.symbol().to_owned();
    }

    fn dump(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width { out.push_str(buf.get(x, y).symbol()); }
            out.push('\n');
        }
        out
    }

    #[test]
    fn very_small_terminal_shows_too_small_message() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 8));
        let state = AppState::new("my_app".into(), "debug".into());
        render(Rect::new(0, 0, 30, 8), &mut buf, &state, &Theme::TOKYO_NIGHT);
        let text = dump(&buf);
        assert!(text.contains("too small"), "missing too-small message, got:\n{text}");
    }

    #[test]
    fn narrow_terminal_uses_vertical_stack() {
        // 70-wide is below NARROW_WIDTH (90) → Performance/Devices stack
        // vertically. Logs are NOT in the inline viewport any more (they
        // flow into the terminal's scrollback via print_above_viewport),
        // so the only two panels we expect to find are these two.
        let mut buf = Buffer::empty(Rect::new(0, 0, 70, 30));
        let state = AppState::new("my_app".into(), "debug".into());
        render(Rect::new(0, 0, 70, 30), &mut buf, &state, &Theme::TOKYO_NIGHT);
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
        render(Rect::new(0, 0, 60, 20), &mut buf, &state, &Theme::TOKYO_NIGHT);
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
        render(Rect::new(0, 0, 120, 24), &mut buf, &state, &Theme::TOKYO_NIGHT);
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
        render(Rect::new(0, 0, 100, 24), &mut buf, &state, &Theme::TOKYO_NIGHT);
        let text = dump(&buf);
        assert!(text.contains('⏱'), "missing chrono icon, got:\n{text}");
        assert!(text.contains("00:00"), "missing elapsed time, got:\n{text}");
    }

    #[test]
    fn header_chrono_switches_to_checkmark_after_compile_finishes() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 24));
        let mut state = AppState::new("my_app".into(), "debug".into());
        state.apply(fl_core::AppEvent::Flutter(fl_core::FlutterEvent::AppStarted {
            app_id: "x".into(),
            vm_service_uri: "ws://x".into(),
        }));
        render(Rect::new(0, 0, 100, 24), &mut buf, &state, &Theme::TOKYO_NIGHT);
        let text = dump(&buf);
        assert!(text.contains('✓'), "expected checkmark after AppStarted, got:\n{text}");
        assert!(!text.contains('⏱'), "chrono running icon should be gone, got:\n{text}");
    }

    #[test]
    fn dashboard_snapshot() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 24));
        let mut state = AppState::new("my_app".into(), "debug".into());
        state.apply(fl_core::AppEvent::Flutter(fl_core::FlutterEvent::Log {
            level: fl_core::LogLevel::Info,
            message: "App started".into(),
        }));
        render(Rect::new(0, 0, 100, 24), &mut buf, &state, &Theme::TOKYO_NIGHT);
        let dump = dump_buffer(&buf);
        insta::assert_snapshot!(dump);
    }

    fn dump_buffer(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf.get(x, y).symbol());
            }
            out.push('\n');
        }
        out
    }
}
