//! Performance panel: FPS sparkline, frame budget, memory sparkline, rebuild rate.

use crate::app::AppState;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

const BLOCKS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

fn sparkline(samples: &std::collections::VecDeque<f32>, max: f32, width: usize) -> String {
    if samples.is_empty() {
        return " ".repeat(width);
    }
    let n = samples.len().min(width);
    let mut out = String::with_capacity(n);
    for v in samples.iter().rev().take(n).rev() {
        let t = (v / max).clamp(0.0, 1.0);
        let idx = ((t * (BLOCKS.len() - 1) as f32).round() as usize).min(BLOCKS.len() - 1);
        out.push(BLOCKS[idx]);
    }
    if out.chars().count() < width {
        for _ in 0..(width - out.chars().count()) {
            out.insert(0, ' ');
        }
    }
    out
}

fn fps_color(fps: f32, theme: &Theme) -> ratatui::style::Color {
    if fps >= 55.0 { theme.success }
    else if fps >= 30.0 { theme.warn }
    else { theme.error }
}

pub fn render_performance(area: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    let block = Block::default()
        .title(" Performance ")
        .borders(Borders::ALL)
        .border_style(theme.dimmed())
        .style(theme.base());
    let inner = block.inner(area);
    block.render(area, buf);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let cur_fps = state.fps_samples.back().copied().unwrap_or(0.0);
    let spark_fps = sparkline(&state.fps_samples, 60.0, (inner.width as usize).saturating_sub(14));
    let fps_line = Line::styled(
        format!("FPS    {spark_fps} {cur_fps:>4.1}"),
        Style::default().fg(fps_color(cur_fps, theme)).bg(theme.bg),
    );
    Paragraph::new(fps_line).render(layout[0], buf);

    let frame_line = Line::styled(
        format!("Frame  ui {:>4.1}ms  raster {:>4.1}ms", state.frame_ui_ms, state.frame_raster_ms),
        theme.dimmed(),
    );
    Paragraph::new(frame_line).render(layout[1], buf);

    let mem_max = state.mem_samples.iter().cloned().fold(64.0_f32, f32::max);
    let cur_mem = state.mem_samples.back().copied().unwrap_or(0.0);
    let spark_mem = sparkline(&state.mem_samples, mem_max, (inner.width as usize).saturating_sub(14));
    let mem_line = Line::styled(
        format!("Memory {spark_mem} {cur_mem:>4.0}MB"),
        theme.base(),
    );
    Paragraph::new(mem_line).render(layout[2], buf);

    let rb = Line::styled(
        format!("Rebuilds {}/s", state.rebuilds_per_sec),
        theme.dimmed(),
    );
    Paragraph::new(rb).render(layout[3], buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn sparkline_emits_one_block_per_sample_capped_to_width() {
        let mut d = VecDeque::new();
        for i in 0..30 { d.push_back(i as f32); }
        let s = sparkline(&d, 30.0, 10);
        assert_eq!(s.chars().count(), 10);
        let last = s.chars().last().unwrap();
        assert_eq!(last, '█');
    }

    #[test]
    fn fps_color_thresholds() {
        let t = Theme::TOKYO_NIGHT;
        assert_eq!(fps_color(60.0, &t), t.success);
        assert_eq!(fps_color(40.0, &t), t.warn);
        assert_eq!(fps_color(15.0, &t), t.error);
    }
}
