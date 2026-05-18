//! Logs panel with filtering and level coloring.

use crate::app::{AppState, LogLine};
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

pub fn render_logs(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    let block = Block::default()
        .title(" Logs ")
        .borders(Borders::ALL)
        .border_style(theme.dimmed())
        .style(theme.base());

    let inner = block.inner(area);
    block.render(area, buf);

    let filter = state.log_filter.as_deref();
    let verbose = state.verbose;
    let visible: Vec<&LogLine> = state
        .logs
        .iter()
        .filter(|l| {
            // Hide DEBUG/TRACE unless verbose is on.
            if !verbose
                && matches!(
                    l.level,
                    fl_core::LogLevel::Debug | fl_core::LogLevel::Trace
                )
            {
                return false;
            }
            match filter {
                Some(f) => l.message.contains(f),
                None => true,
            }
        })
        .collect();

    let take = inner.height as usize;
    let slice: Vec<&LogLine> = visible.iter().rev().take(take).rev().copied().collect();

    let lines: Vec<Line> = slice.iter().map(|l| {
        let prefix = match l.level {
            fl_core::LogLevel::Error => "ERROR ",
            fl_core::LogLevel::Warn => "WARN  ",
            fl_core::LogLevel::Info => "INFO  ",
            fl_core::LogLevel::Debug => "DEBUG ",
            fl_core::LogLevel::Trace => "TRACE ",
        };
        Line::styled(format!("{prefix}{}", l.message), theme.level(l.level))
    }).collect();

    Paragraph::new(lines)
        .style(theme.base())
        .render(inner, buf);
}

pub fn measure_visible(state: &AppState) -> usize {
    let verbose = state.verbose;
    state
        .logs
        .iter()
        .filter(|l| {
            if !verbose
                && matches!(
                    l.level,
                    fl_core::LogLevel::Debug | fl_core::LogLevel::Trace
                )
            {
                return false;
            }
            match state.log_filter.as_deref() {
                Some(f) => l.message.contains(f),
                None => true,
            }
        })
        .count()
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
        s.apply(AppEvent::Flutter(FlutterEvent::Log { level: LogLevel::Info, message: "alpha".into() }));
        s.apply(AppEvent::Flutter(FlutterEvent::Log { level: LogLevel::Info, message: "beta".into() }));
        s.log_filter = Some("alp".into());
        assert_eq!(measure_visible(&s), 1);
    }
}
