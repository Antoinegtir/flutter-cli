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

    let n = state.active_sessions.len();
    if n <= 1 {
        render_single(inner, buf, state, theme);
    } else {
        render_summary(inner, buf, state, theme, n);
    }
}

fn render_single(inner: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let w = inner.width as usize;
    let cur_fps = state.fps_samples.back().copied().unwrap_or(0.0);
    let n_samples = state.fps_samples.len();
    let avg_fps = if n_samples == 0 {
        0.0
    } else {
        state.fps_samples.iter().copied().sum::<f32>() / n_samples as f32
    };
    let max_fps = state
        .fps_samples
        .iter()
        .cloned()
        .fold(0.0_f32, f32::max);
    let real_fps = state.frames_per_sec();
    let jank_pct = state.jank_ratio() * 100.0;

    // Line 1 — FPS sparkline with current value on the right.
    let spark_fps = sparkline(&state.fps_samples, 60.0, w.saturating_sub(13));
    let fps_line = Line::styled(
        format!("FPS    {spark_fps} {cur_fps:>5.1}"),
        Style::default().fg(fps_color(cur_fps, theme)).bg(theme.bg),
    );
    Paragraph::new(fps_line).render(layout[0], buf);

    // Line 2 — frame phase timings + jank ratio. Drops gracefully if the
    // panel is narrow.
    let frame_line_text = if w >= 44 {
        format!(
            "Frame  ui {:>4.1}ms  raster {:>4.1}ms  jank {:>3.0}%",
            state.frame_ui_ms, state.frame_raster_ms, jank_pct
        )
    } else {
        format!(
            "Frame  ui {:>4.1}  raster {:>4.1}  J{:>3.0}%",
            state.frame_ui_ms, state.frame_raster_ms, jank_pct
        )
    };
    Paragraph::new(Line::styled(frame_line_text, theme.dimmed())).render(layout[1], buf);

    // Line 3 — memory sparkline + used / capacity.
    let mem_max = state
        .mem_samples
        .iter()
        .cloned()
        .fold(state.heap_capacity_mb.max(64.0), f32::max);
    let cur_mem = state.mem_samples.back().copied().unwrap_or(0.0);
    let mem_label = if state.heap_capacity_mb > 0.0 {
        format!("{cur_mem:>4.0}/{:>4.0}MB", state.heap_capacity_mb)
    } else {
        format!("{cur_mem:>4.0}MB")
    };
    let spark_mem = sparkline(
        &state.mem_samples,
        mem_max,
        w.saturating_sub(8 + mem_label.chars().count()),
    );
    Paragraph::new(Line::styled(
        format!("Memory {spark_mem} {mem_label}"),
        theme.base(),
    ))
    .render(layout[2], buf);

    // Line 4 — averaged FPS over the sample window + actual frame rate.
    let avg_line = if w >= 38 {
        format!("Avg  {avg_fps:>4.1}fps  rate {real_fps:>3.0}/s  peak {max_fps:>4.1}")
    } else {
        format!("Avg {avg_fps:>4.1}  rate {real_fps:>3.0}/s")
    };
    Paragraph::new(Line::styled(avg_line, theme.dimmed())).render(layout[3], buf);

    // Line 5 — cumulative frame count, a steady signal you can stopwatch.
    Paragraph::new(Line::styled(
        format!("Frames {} since start", state.total_frames),
        theme.dimmed(),
    ))
    .render(layout[4], buf);
}

fn render_summary(inner: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme, n: usize) {
    let cur_fps = state.fps_samples.back().copied().unwrap_or(0.0);
    let spark_fps = sparkline(&state.fps_samples, 60.0, 8);
    let line1 = Line::styled(
        format!("FPS avg {spark_fps} {cur_fps:>4.1}  ·  {n} devices online"),
        Style::default().fg(fps_color(cur_fps, theme)).bg(theme.bg),
    );
    let cur_mem = state.mem_samples.back().copied().unwrap_or(0.0);
    let line2 = Line::styled(
        format!("Mem ~{cur_mem:.0}MB total"),
        theme.dimmed(),
    );
    Paragraph::new(vec![line1, line2]).render(inner, buf);
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

    #[test]
    fn renders_summary_when_two_or_more_sessions() {
        use crate::app::AppState;
        use fl_core::{AppEvent, DeviceEvent, DeviceSessionState};

        let mut s = AppState::new("a".into(), "d".into());
        for serial in ["a", "b", "c"] {
            s.apply(AppEvent::Device(DeviceEvent::SessionState {
                serial: serial.into(),
                state: DeviceSessionState::Ready,
            }));
        }
        // Add some sample data so sparkline has content
        s.fps_samples.push_back(30.0);
        s.fps_samples.push_back(35.0);
        s.mem_samples.push_back(128.0);

        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 5));
        render_performance(Rect::new(0, 0, 80, 5), &mut buf, &s, &Theme::TOKYO_NIGHT);
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width { text.push_str(buf.get(x, y).symbol()); }
            text.push('\n');
        }
        assert!(text.contains("3 devices"), "missing summary count:\n{text}");
    }
}
