//! Top-level dashboard render: header + body split + footer + optional banner.

use crate::app::{AppState, BannerKind};
use crate::panels;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

const FOOTER: &str = " [r] reload  [R] restart  [b] theme  [p] paint  [o] platform  [w] wifi  [/] filter  [c] clear  [?] help  [q] quit ";

pub fn render(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(area);
    render_header(layout[0], buf, state, theme);
    render_body(layout[1], buf, state, theme);
    render_footer(layout[2], buf, theme);
    if let Some(b) = &state.banner {
        render_banner(area, buf, &b.message, b.kind, theme);
    }
}

fn render_header(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    let device = match state.active_sessions.len() {
        0 => "no device".to_string(),
        1 => state.active_sessions[0].display_name.clone(),
        n => format!("{n} devices"),
    };
    let title = format!(" fl ── {} · {} · {} ", state.app_name, state.mode, device);
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
    Paragraph::new(Line::styled(title, theme.header())).render(inner, buf);
}

fn render_body(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);
    panels::logs::render_logs(cols[0], buf, state, theme);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(cols[1]);
    panels::performance::render_performance(right[0], buf, state, theme);
    panels::devices::render_devices(right[1], buf, state, theme);
}

fn render_footer(area: Rect, buf: &mut Buffer, theme: &Theme) {
    Paragraph::new(Line::styled(FOOTER, theme.dimmed())).render(area, buf);
}

fn render_banner(area: Rect, buf: &mut Buffer, msg: &str, kind: BannerKind, theme: &Theme) {
    let color = match kind {
        BannerKind::Info => theme.cyan,
        BannerKind::Warn => theme.warn,
        BannerKind::Error => theme.error,
        BannerKind::Success => theme.success,
    };
    let line = format!(" {msg} ");
    let target = Rect {
        x: area.x + (area.width.saturating_sub(line.chars().count() as u16)) / 2,
        y: area.y + 1,
        width: line.chars().count() as u16,
        height: 1,
    };
    Paragraph::new(Line::styled(line, Style::default().fg(theme.bg).bg(color)))
        .render(target, buf);
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
