//! Tokyo Night palette + thin wrappers around ratatui `Style`.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub success: Color,
    pub warn: Color,
    pub error: Color,
    pub dim: Color,
    pub cyan: Color,
}

impl Theme {
    pub const TOKYO_NIGHT: Theme = Theme {
        bg: Color::Rgb(0x1a, 0x1b, 0x26),
        fg: Color::Rgb(0xc0, 0xca, 0xf5),
        accent: Color::Rgb(0x7a, 0xa2, 0xf7),
        success: Color::Rgb(0x9e, 0xce, 0x6a),
        warn: Color::Rgb(0xe0, 0xaf, 0x68),
        error: Color::Rgb(0xf7, 0x76, 0x8e),
        dim: Color::Rgb(0x56, 0x5f, 0x89),
        cyan: Color::Rgb(0x7d, 0xcf, 0xff),
    };

    pub fn base(&self) -> Style {
        Style::default().fg(self.fg).bg(self.bg)
    }
    pub fn header(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .bg(self.bg)
            .add_modifier(Modifier::BOLD)
    }
    pub fn dimmed(&self) -> Style {
        Style::default().fg(self.dim).bg(self.bg)
    }
    pub fn level(&self, lvl: fl_core::LogLevel) -> Style {
        let fg = match lvl {
            fl_core::LogLevel::Error => self.error,
            fl_core::LogLevel::Warn => self.warn,
            fl_core::LogLevel::Info => self.cyan,
            fl_core::LogLevel::Debug => self.dim,
            fl_core::LogLevel::Trace => self.dim,
        };
        Style::default().fg(fg).bg(self.bg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fl_core::LogLevel;

    #[test]
    fn error_level_uses_red_palette_color() {
        let s = Theme::TOKYO_NIGHT.level(LogLevel::Error);
        assert_eq!(s.fg, Some(Theme::TOKYO_NIGHT.error));
    }

    #[test]
    fn header_is_bold_accent() {
        let s = Theme::TOKYO_NIGHT.header();
        assert!(s.add_modifier.contains(Modifier::BOLD));
        assert_eq!(s.fg, Some(Theme::TOKYO_NIGHT.accent));
    }
}
