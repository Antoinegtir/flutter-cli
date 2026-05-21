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

    /// Tokyo Night Day — the light counterpart. Dark-on-light colours so
    /// the dashboard and the scrollback logs stay readable on terminals
    /// with a light background (where the NIGHT palette's pale blues
    /// would wash out to near-invisible).
    pub const TOKYO_DAY: Theme = Theme {
        bg: Color::Rgb(0xe1, 0xe2, 0xe7),
        fg: Color::Rgb(0x34, 0x54, 0x8a),
        accent: Color::Rgb(0x2e, 0x7d, 0xe9),
        success: Color::Rgb(0x58, 0x75, 0x39),
        warn: Color::Rgb(0x8c, 0x6c, 0x3e),
        error: Color::Rgb(0xf5, 0x2a, 0x65),
        dim: Color::Rgb(0x84, 0x8c, 0xb5),
        cyan: Color::Rgb(0x00, 0x71, 0x97),
    };

    /// Pick a palette to match the terminal's background. Order:
    ///   1. `FLUTTER_CLI_THEME=light|dark` explicit override.
    ///   2. OSC-11 background query (via `termbg`), 100 ms timeout.
    ///   3. Dark fallback (the historical default, and what most dev
    ///      terminals use) when the query fails or isn't a TTY.
    pub fn detect() -> Theme {
        if let Ok(v) = std::env::var("FLUTTER_CLI_THEME") {
            match v.to_ascii_lowercase().as_str() {
                "light" => return Theme::TOKYO_DAY,
                "dark" => return Theme::TOKYO_NIGHT,
                _ => {}
            }
        }
        match termbg::theme(std::time::Duration::from_millis(100)) {
            Ok(termbg::Theme::Light) => Theme::TOKYO_DAY,
            _ => Theme::TOKYO_NIGHT,
        }
    }

    /// ANSI 24-bit foreground SGR for an `Rgb` palette colour, e.g.
    /// `"\x1b[38;2;122;162;247m"`. Non-`Rgb` colours yield an empty
    /// string (terminal default fg). Used by the raw scrollback writers
    /// (`print_above_viewport`, banner) that bypass ratatui styling.
    pub fn sgr_fg(c: Color) -> String {
        match c {
            Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
            _ => String::new(),
        }
    }

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
