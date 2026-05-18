//! ASCII splash with a single horizontal shimmer sweep.

use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use std::time::Duration;

const LOGO: &[&str] = &[
    "  ███████╗██╗     ",
    "  ██╔════╝██║     ",
    "  █████╗  ██║     ",
    "  ██╔══╝  ██║     ",
    "  ██║     ███████╗",
    "  ╚═╝     ╚══════╝",
];

const SWEEP_MS: u64 = 800;

pub struct Splash {
    theme: Theme,
    pub elapsed: Duration,
}

impl Splash {
    pub fn new(theme: Theme) -> Self {
        Self { theme, elapsed: Duration::ZERO }
    }
    pub fn tick(&mut self, dt: Duration) {
        self.elapsed = self.elapsed.saturating_add(dt);
    }
    pub fn done(&self) -> bool {
        self.elapsed.as_millis() as u64 >= SWEEP_MS + 200
    }
}

impl Widget for &Splash {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let total_cols = LOGO[0].chars().count() as f32;
        let progress = (self.elapsed.as_millis() as f32 / SWEEP_MS as f32).clamp(0.0, 1.0);
        let head = progress * (total_cols + 8.0);

        let lines: Vec<Line> = LOGO.iter().map(|row| {
            let spans: Vec<Span> = row.chars().enumerate().map(|(i, c)| {
                let col = i as f32;
                let dist = (head - col).abs();
                let lerp_t = (1.0 - (dist / 6.0)).clamp(0.0, 1.0);
                let color = lerp_color(self.theme.dim, self.theme.accent, lerp_t);
                Span::styled(c.to_string(), Style::default().fg(color).bg(self.theme.bg))
            }).collect();
            Line::from(spans)
        }).collect();

        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .render(area, buf);
    }
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let (ar, ag, ab) = rgb_or(a, (0x56, 0x5f, 0x89));
    let (br, bg, bb) = rgb_or(b, (0x7a, 0xa2, 0xf7));
    let lerp = |a: u8, b: u8| -> u8 {
        let af = a as f32;
        let bf = b as f32;
        (af + (bf - af) * t).round().clamp(0.0, 255.0) as u8
    };
    Color::Rgb(lerp(ar, br), lerp(ag, bg), lerp(ab, bb))
}

fn rgb_or(c: Color, fallback: (u8, u8, u8)) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn splash_is_done_after_sweep_plus_settle() {
        let mut s = Splash::new(Theme::TOKYO_NIGHT);
        assert!(!s.done());
        s.tick(Duration::from_millis(SWEEP_MS + 250));
        assert!(s.done());
    }

    #[test]
    fn lerp_at_0_returns_a_and_at_1_returns_b() {
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(100, 200, 50);
        assert_eq!(lerp_color(a, b, 0.0), a);
        assert_eq!(lerp_color(a, b, 1.0), b);
    }

    #[test]
    fn splash_renders_without_panicking() {
        let s = Splash::new(Theme::TOKYO_NIGHT);
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 8));
        (&s).render(Rect::new(0, 0, 60, 8), &mut buf);
    }
}
