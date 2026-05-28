//! Logs panel with filtering and level coloring.

use crate::app::{AppState, LogLine};
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

/// Level name as a lowercase static str — no allocation.
fn level_name(level: fl_core::LogLevel) -> &'static str {
    match level {
        fl_core::LogLevel::Error => "error",
        fl_core::LogLevel::Warn => "warn",
        fl_core::LogLevel::Info => "info",
        fl_core::LogLevel::Debug => "debug",
        fl_core::LogLevel::Trace => "trace",
    }
}

/// ASCII case-insensitive substring search. Avoids allocating a
/// lowercased copy of `haystack` on every render frame — at 30fps with
/// 1000 logs the previous `to_ascii_lowercase` was burning ~30k
/// allocations / sec just to filter.
fn contains_ascii_ci(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if needle_lower.len() > haystack.len() {
        return false;
    }
    let hb = haystack.as_bytes();
    let nb = needle_lower.as_bytes();
    'outer: for i in 0..=(hb.len() - nb.len()) {
        for j in 0..nb.len() {
            if hb[i + j].to_ascii_lowercase() != nb[j] {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

/// Filter match: case-insensitive, and matches against the level name
/// (`error`/`warn`/`info`/`debug`/`trace`) as well as the message body.
/// The needle has already been lowercased by the caller (once per
/// render, instead of once per log line).
fn matches_filter(line: &LogLine, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    level_name(line.level).contains(needle_lower) || contains_ascii_ci(&line.message, needle_lower)
}

/// Render a single log line as a ratatui `Line`. Single span, no
/// file-ref highlighting — see `runner.rs::log_style_for` for the
/// level-based coloring that's actually used in the inline-viewport
/// scrollback (the panels-based logs view kept here is currently
/// dead code; we keep it minimal in case it comes back).
fn render_log_line(l: &LogLine, theme: &Theme) -> Line<'static> {
    let prefix = match l.level {
        fl_core::LogLevel::Error => "ERROR ",
        fl_core::LogLevel::Warn => "WARN  ",
        fl_core::LogLevel::Info => "INFO  ",
        fl_core::LogLevel::Debug => "DEBUG ",
        fl_core::LogLevel::Trace => "TRACE ",
    };
    Line::styled(format!("{prefix}{}", l.message), theme.level(l.level))
}

pub fn render_logs(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    // Pre-lowercase the filter ONCE. The hot path below would otherwise
    // do this per log per frame.
    let filter_lower = state.log_filter.as_deref().map(str::to_ascii_lowercase);

    // The block borders consume 2 rows, so inner.height tells us how many
    // log lines we can actually paint. Publish it back to AppState so the
    // key handler can clamp Up/PageUp at the oldest line instead of letting
    // the viewport collapse to a single row.
    let viewport_outer = area.height.saturating_sub(2) as usize;
    state
        .log_viewport_height
        .store(viewport_outer.max(1), std::sync::atomic::Ordering::Relaxed);

    // Count matching logs once (cheap — no allocations).
    let n: usize = match filter_lower.as_deref() {
        Some(f) => state.logs.iter().filter(|l| matches_filter(l, f)).count(),
        None => state.logs.len(),
    };

    let max_off = n.saturating_sub(viewport_outer);
    let off = state.log_scroll_offset.min(max_off);

    let title = if off > 0 {
        format!(" Logs · paused -{off} / {n} (g=tail, G=top) ")
    } else {
        format!(" Logs · {n} ")
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(theme.dimmed())
        .style(theme.base());

    let inner = block.inner(area);
    block.render(area, buf);

    let viewport = inner.height as usize;
    if viewport == 0 {
        return;
    }

    // Collect ONLY the visible window. We walk logs from newest to oldest
    // (rev), skip `off` (scrolled-into-history offset), take `viewport`,
    // then reverse to get chronological order. Allocates at most
    // `viewport` &LogLine refs (typically 20-40) regardless of how
    // many logs are in the ring.
    let mut window: Vec<&LogLine> = match filter_lower.as_deref() {
        Some(f) => state
            .logs
            .iter()
            .rev()
            .filter(|l| matches_filter(l, f))
            .skip(off)
            .take(viewport)
            .collect(),
        None => state.logs.iter().rev().skip(off).take(viewport).collect(),
    };
    window.reverse();

    let lines: Vec<Line> = window
        .iter()
        .map(|l| render_log_line(l, theme))
        .collect();

    Paragraph::new(lines).style(theme.base()).render(inner, buf);
}

pub fn measure_visible(state: &AppState) -> usize {
    match state.log_filter.as_deref() {
        Some(f) => {
            let lower = f.to_ascii_lowercase();
            state
                .logs
                .iter()
                .filter(|l| matches_filter(l, &lower))
                .count()
        }
        None => state.logs.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fl_core::{AppEvent, FlutterEvent, LogLevel};

    #[test]
    fn empty_filter_shows_all() {
        let mut s = AppState::new("a".into(), "d".into());
        for i in 0..3 {
            s.apply(AppEvent::Flutter(FlutterEvent::Log {
                level: LogLevel::Info,
                message: format!("hello {i}"),
            }));
        }
        assert_eq!(measure_visible(&s), 3);
    }

    #[test]
    fn filter_substring_reduces_visible_count() {
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(AppEvent::Flutter(FlutterEvent::Log {
            level: LogLevel::Info,
            message: "alpha".into(),
        }));
        s.apply(AppEvent::Flutter(FlutterEvent::Log {
            level: LogLevel::Info,
            message: "beta".into(),
        }));
        s.log_filter = Some("alp".into());
        assert_eq!(measure_visible(&s), 1);
    }

    #[test]
    fn filter_matches_level_name_case_insensitive() {
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(AppEvent::Flutter(FlutterEvent::Log {
            level: LogLevel::Debug,
            message: "x".into(),
        }));
        s.apply(AppEvent::Flutter(FlutterEvent::Log {
            level: LogLevel::Info,
            message: "y".into(),
        }));
        s.apply(AppEvent::Flutter(FlutterEvent::Log {
            level: LogLevel::Error,
            message: "z".into(),
        }));
        s.log_filter = Some("DEBUG".into());
        assert_eq!(measure_visible(&s), 1);
        s.log_filter = Some("error".into());
        assert_eq!(measure_visible(&s), 1);
    }
}
