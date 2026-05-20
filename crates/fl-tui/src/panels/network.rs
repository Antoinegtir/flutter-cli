//! Network inspector panel — live HTTP requests captured from the
//! running Dart VM(s) via `ext.dart.io.getHttpProfile`. Rendered as
//! a scrolling table: time · method · URL · status · duration.
//!
//! Toggled in/out via the `n` keybind; the data collection runs in
//! the background regardless of panel visibility so flipping `n`
//! later shows the full history that was captured while the panel
//! was hidden.

use crate::app::AppState;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

pub fn render_network(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    let block = Block::default()
        .title(" Network ")
        .borders(Borders::ALL)
        .border_style(theme.dimmed())
        .style(theme.base());
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 {
        return;
    }

    // Empty-state copy. Shows the user what's about to happen so an
    // unexpectedly empty panel doesn't look broken.
    if state.network_requests.is_empty() {
        let lines = vec![
            Line::styled("No HTTP traffic captured yet.", theme.dimmed()),
            Line::styled(
                "Will populate as the app makes requests via `dart:io` or `package:http`.",
                theme.dimmed(),
            ),
        ];
        Paragraph::new(lines).render(inner, buf);
        return;
    }

    // Column layout. We compute widths up front so each cell can
    // truncate independently — saves a lot of `if w < 60` branching.
    let w = inner.width as usize;
    let method_w = 6; // GET / POST / PATCH / DELETE …
    let status_w = 4; // 200 / 404 / 500
    let dur_w = 7; // "  123ms"
    let hint_w = 3; // "  ↑↓" right-anchored on the header
    let sep = 3 * 1; // three single spaces between 4 columns
    let url_w = w.saturating_sub(
        method_w + status_w + dur_w + sep + hint_w + 2, /* slack */
    );

    let mut lines: Vec<Line> = Vec::with_capacity(inner.height as usize);

    // Header row. The `↑↓` hint sits at the top-right corner of
    // the panel to signal the panel is scrollable without taking up
    // the leftmost column where the eye actually reads the data.
    lines.push(Line::styled(
        format!(
            "{:<m$} {:<u$} {:>s$} {:>d$}  ↑↓",
            "method",
            "url",
            "code",
            "ms",
            m = method_w,
            u = url_w,
            s = status_w,
            d = dur_w
        ),
        theme.dimmed(),
    ));

    // Compute the visible window honouring the user's scroll offset
    // (Up arrow walks back through history while in this panel).
    //   tail_skip = how many of the newest requests to hide.
    //   from     = first index of the window we display.
    //   to       = exclusive end of the window.
    let cap = (inner.height as usize).saturating_sub(1);
    // Publish the row capacity so `on_key`'s Up handler can clamp
    // `network_scroll_offset` to the oldest visible row instead of
    // pushing past the buffer's head.
    state
        .network_viewport_height
        .store(cap.max(1), std::sync::atomic::Ordering::Relaxed);
    let total = state.network_requests.len();
    // Cap defensively too — covers the case where the key handler
    // ran with a stale viewport height (window resize between draws).
    let max_skip = total.saturating_sub(cap.max(1));
    let tail_skip = state.network_scroll_offset.min(max_skip);
    let to = total - tail_skip;
    let from = to.saturating_sub(cap);
    // When the user has scrolled back, show a hint in the header so
    // they know they're not seeing live traffic any more.
    if tail_skip > 0 {
        if let Some(last) = lines.last_mut() {
            *last = Line::styled(
                format!(
                    "{:<m$} {:<u$} {:>s$} {:>d$}  ↑↓  scroll {} (Down to follow live)",
                    "method",
                    "url",
                    "code",
                    "ms",
                    tail_skip,
                    m = method_w,
                    u = url_w,
                    s = status_w,
                    d = dur_w,
                ),
                theme.dimmed(),
            );
        }
    }
    for req in state.network_requests.iter().skip(from).take(to - from) {
        let url = truncate(&req.url, url_w);
        let status_str = match req.status {
            Some(s) => format!("{s}"),
            None => "…".to_string(),
        };
        let dur_str = match req.duration_ms {
            Some(d) => format!("{d}ms"),
            None => "…".to_string(),
        };
        // Header has the `↑↓` hint on the right; data rows leave
        // that trailing space empty so the columns line up under
        // their respective titles.
        let line = format!(
            "{:<m$} {:<u$} {:>s$} {:>d$}",
            req.method,
            url,
            status_str,
            dur_str,
            m = method_w,
            u = url_w,
            s = status_w,
            d = dur_w
        );
        lines.push(Line::styled(
            line,
            Style::default()
                .fg(status_color(req.status, theme))
                .bg(theme.bg),
        ));
    }
    Paragraph::new(lines).render(inner, buf);
}

/// Colour a row by HTTP status family: 2xx green, 3xx fg, 4xx warn,
/// 5xx error, in-flight dim. Mirrors Chrome DevTools' Network tab.
fn status_color(status: Option<u16>, theme: &Theme) -> Color {
    match status {
        Some(s) if (200..300).contains(&s) => theme.success,
        Some(s) if (300..400).contains(&s) => theme.fg,
        Some(s) if (400..500).contains(&s) => theme.warn,
        Some(s) if s >= 500 => theme.error,
        _ => theme.dim,
    }
}

fn truncate(s: &str, max: usize) -> String {
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
