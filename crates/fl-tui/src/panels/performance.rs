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
    if fps >= 55.0 {
        theme.success
    } else if fps >= 30.0 {
        theme.warn
    } else {
        theme.error
    }
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
        // 0 or 1 device: use the global perf state (preserves existing
        // single-device layout and keeps headless / pre-pair-VM cases
        // looking like before).
        render_single(inner, buf, state, theme);
    } else {
        render_per_device(inner, buf, state, theme);
    }
}

/// Multi-device: stack one compact 3-row block per active device, with
/// a 1-row separator label showing the device name. Each block reuses
/// the device's own `DevicePerf` so the sparklines are real per-device
/// traces and not a confusing merged view.
fn render_per_device(inner: Rect, buf: &mut Buffer, state: &AppState, theme: &Theme) {
    let sessions = &state.active_sessions;
    let n = sessions.len() as u16;
    if n == 0 || inner.height == 0 {
        return;
    }
    // Each device takes 3 rows: name header + FPS line + Memory line.
    // If there aren't enough rows for everyone, devices share what's
    // available evenly (minimum 2 rows each); the renderer below
    // gracefully drops the memory line if it only gets 2 rows.
    let per_dev_h = (inner.height / n).max(2);
    let mut y = inner.y;
    for (i, sess) in sessions.iter().enumerate() {
        if y >= inner.y + inner.height {
            break;
        }
        let h = if i + 1 == sessions.len() {
            (inner.y + inner.height).saturating_sub(y)
        } else {
            per_dev_h.min((inner.y + inner.height).saturating_sub(y))
        };
        let cell = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: h,
        };
        render_one_device(cell, buf, state, sess, theme);
        y = y.saturating_add(h);
    }
}

fn render_one_device(
    cell: Rect,
    buf: &mut Buffer,
    state: &AppState,
    sess: &fl_core::DeviceSessionSummary,
    theme: &Theme,
) {
    let perf = state.device_perf.get(&sess.serial);
    let fps_samples = perf.map(|p| &p.fps_samples);
    let mem_samples = perf.map(|p| &p.mem_samples);
    let cap = perf.map(|p| p.heap_capacity_mb).unwrap_or(0.0);
    let cur_fps = fps_samples.and_then(|s| s.back().copied()).unwrap_or(0.0);
    let cur_mem = mem_samples.and_then(|s| s.back().copied()).unwrap_or(0.0);

    let w = cell.width as usize;

    // Row 1: device name (so the user can tell which block is which).
    let name = format!("· {}", sess.display_name);
    Paragraph::new(Line::styled(name, theme.dimmed())).render(
        Rect {
            x: cell.x,
            y: cell.y,
            width: cell.width,
            height: 1,
        },
        buf,
    );

    // Row 2: FPS sparkline + current value.
    if cell.height >= 2 {
        let empty = std::collections::VecDeque::<f32>::new();
        let samples = fps_samples.unwrap_or(&empty);
        let spark = sparkline(samples, 60.0, w.saturating_sub(13));
        Paragraph::new(Line::styled(
            format!("FPS    {spark} {cur_fps:>5.1}"),
            Style::default().fg(fps_color(cur_fps, theme)).bg(theme.bg),
        ))
        .render(
            Rect {
                x: cell.x,
                y: cell.y + 1,
                width: cell.width,
                height: 1,
            },
            buf,
        );
    }

    // Row 3: memory sparkline + used/total.
    if cell.height >= 3 {
        let mem_max = mem_samples
            .map(|s| s.iter().copied().fold(cap.max(64.0), f32::max))
            .unwrap_or(64.0);
        let mem_label = if cap > 0.0 {
            format!("{cur_mem:>4.0}/{cap:>4.0}MB")
        } else {
            format!("{cur_mem:>4.0}MB")
        };
        let empty = std::collections::VecDeque::<f32>::new();
        let samples = mem_samples.unwrap_or(&empty);
        let spark = sparkline(
            samples,
            mem_max,
            w.saturating_sub(8 + mem_label.chars().count()),
        );
        Paragraph::new(Line::styled(
            format!("Memory {spark} {mem_label}"),
            theme.base(),
        ))
        .render(
            Rect {
                x: cell.x,
                y: cell.y + 2,
                width: cell.width,
                height: 1,
            },
            buf,
        );
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
    let max_fps = state.fps_samples.iter().cloned().fold(0.0_f32, f32::max);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn sparkline_emits_one_block_per_sample_capped_to_width() {
        let mut d = VecDeque::new();
        for i in 0..30 {
            d.push_back(i as f32);
        }
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
    fn renders_one_block_per_session_with_two_or_more_devices() {
        use crate::app::{AppState, DevicePerf};
        use fl_core::{AppEvent, DeviceEvent, DeviceSessionState};

        let mut s = AppState::new("a".into(), "d".into());
        for serial in ["alpha", "beta", "gamma"] {
            s.apply(AppEvent::Device(DeviceEvent::SessionState {
                serial: serial.into(),
                state: DeviceSessionState::Ready,
            }));
            // Seed per-device perf so the rendered block has live data.
            let mut p = DevicePerf::default();
            p.fps_samples.push_back(30.0);
            p.mem_samples.push_back(128.0);
            s.device_perf.insert(serial.into(), p);
        }
        // Tall enough for 3 blocks × 3 rows each.
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 12));
        render_performance(Rect::new(0, 0, 80, 12), &mut buf, &s, &Theme::TOKYO_NIGHT);
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[(x, y)].symbol());
            }
            text.push('\n');
        }
        // Each device's name header should appear once.
        assert!(text.contains("alpha"), "missing alpha block:\n{text}");
        assert!(text.contains("beta"), "missing beta block:\n{text}");
        assert!(text.contains("gamma"), "missing gamma block:\n{text}");
        // And the FPS/Memory rows should be present (at least once each).
        assert!(text.contains("FPS"), "missing FPS row:\n{text}");
        assert!(text.contains("Memory"), "missing Memory row:\n{text}");
    }
}
