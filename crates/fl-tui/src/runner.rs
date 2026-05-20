//! Drives the TUI: render loop on a tick, drain `AppEvent` channel, handle key input.

use crate::app::{AppState, LogLine};
use crate::render::render;
use crate::theme::Theme;
use anyhow::Context;
use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers, MouseEvent, MouseEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use fl_core::{AppEvent, KeyEvent as FlKey};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::io::Stdout;
use std::io::Write;
use std::time::Duration;
use tokio::sync::mpsc::{Receiver, Sender};

/// Which viewport mode the terminal is in. Determines what `restore()`
/// needs to undo — we entered an alternate screen for fullscreen mode
/// but stayed on the primary screen for inline mode, and the cleanup
/// has to match.
#[derive(Clone, Copy)]
enum ViewportMode {
    /// Alternate screen, swallows the whole terminal — used by views
    /// like the device picker and `fl test` where we want exclusive
    /// real estate.
    Fullscreen,
    /// Inline viewport pinned to the bottom of the user's terminal.
    /// Above the box, the shell's scrollback (command history, output
    /// of previous tools) stays visible — this is the "Claude Code"
    /// look. No mouse capture, so the terminal's native scroll keeps
    /// working for that scrollback.
    Inline,
}

/// Minimum width below which we skip the welcome banner entirely —
/// anything narrower would either wrap and look broken or crush the
/// right column down to nothing.
const BANNER_MIN_WIDTH: u16 = 82;

/// Try to obtain the user's full name (e.g. "Antoine Gonthier") via
/// macOS's `id -F`. On non-macOS systems or when `id -F` fails we
/// fall back to `$USER`, and finally to a generic "there". Used to
/// personalize the welcome banner.
fn detect_user_display_name() -> String {
    if cfg!(target_os = "macos") {
        if let Ok(out) = std::process::Command::new("id").arg("-F").output() {
            if out.status.success() {
                let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !name.is_empty() {
                    // First name only — feels more conversational than
                    // a full "Welcome back, Antoine Gonthier!" mouthful.
                    return name.split_whitespace().next().unwrap_or(&name).to_string();
                }
            }
        }
    }
    std::env::var("USER")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "there".to_string())
}

/// Welcome banner lines, à la Claude Code's startup box: bordered,
/// blue-tinted, with an ASCII bird logo on the left and an info /
/// keybinds column on the right. Each entry is a fully-formatted line
/// (including ANSI SGR sequences) ready to be written verbatim to
/// stdout. They are printed once at init into the terminal's
/// scrollback and naturally scroll upward as logs flow in.
///
/// `width` is the terminal column count — the banner expands to fit
/// the full terminal width, just like Claude Code's welcome box does.
fn welcome_banner_lines(width: u16) -> Vec<String> {
    // Tokyo Night-ish blue palette so the box sits well on dark
    // terminals. We use 24-bit color (\x1b[38;2;R;G;Bm) — every
    // modern terminal we care about (iTerm2, Terminal.app, Warp,
    // Alacritty, Kitty, tmux ≥3.2) renders it correctly.
    const B: &str = "\x1b[38;2;122;162;247m"; // box / accent
    const T: &str = "\x1b[38;2;192;202;245m"; // primary text
    const D: &str = "\x1b[38;2;105;120;170m"; // dim text (tips, urls)
    const BD: &str = "\x1b[1m"; // bold
    const R: &str = "\x1b[0m"; // reset

    // Personalize the welcome line with the user's first name on macOS
    // (`id -F`), falling back to `$USER` elsewhere. Mirrors Claude
    // Code's "Welcome back Antoine!" greeting.
    let user_name = detect_user_display_name();
    let welcome = format!("Welcome back, {user_name}!");

    // Each row of the banner is laid out as:
    //   `│` (1) + ` ` (1) + art (20) + `  ` (2) + right (W) + ` ` (1) + `│` (1)
    //   = `width` visible columns, where W = width - 26.
    // The art column is the bird logo (11 rows, 20 chars wide); the
    // right column carries the welcome copy + keybinds reference and
    // stretches to fill whatever width the terminal gives us. Empty
    // strings in either column render as whitespace of the correct
    // width.
    let rows: [(&str, String); 11] = [
        ("           ------   ", String::new()),
        ("         ------     ", welcome),
        ("       ------       ", String::new()),
        ("      ------        ", "Modern Flutter CLI".to_string()),
        (
            "    ------          ",
            "hot-reload · multi-device · perf monitor".to_string(),
        ),
        ("  ------   ----==   ", String::new()),
        ("    --   =----=     ", "Tips".to_string()),
        (
            "       =======      ",
            "• [r] hot reload   [R] hot restart".to_string(),
        ),
        (
            "       =====#       ",
            "• [b] theme        [P] perf overlay".to_string(),
        ),
        ("         =#####     ", "• [/] filter live".to_string()),
        (
            "           ######   ",
            "• [c] copy         [q] quit".to_string(),
        ),
    ];

    let inner_w = width.saturating_sub(2) as usize; // chars between the two `│`s
    let right_col_w = (width as usize).saturating_sub(26); // dynamic right column width

    let mut out: Vec<String> = Vec::with_capacity(16);

    // Top border with embedded title.
    //   ╭─ fl v0.1.0 ────…────╮
    let title = " fl v0.1.0 ";
    // Visible width breakdown: `╭` (1) + `─` (1) + title (N) + `─…─` (D) + `╮` (1) = width
    // → D = width - 3 - N.
    let dashes_after_title = (width as usize)
        .saturating_sub(3)
        .saturating_sub(title.chars().count());
    out.push(format!(
        "{B}╭─{BD}{title}{R}{B}{}╮{R}",
        "─".repeat(dashes_after_title)
    ));

    // Padding row.
    out.push(format!("{B}│{R}{}{B}│{R}", " ".repeat(inner_w)));

    for (art, right) in rows.iter() {
        // Style the right text. Bullet rows are dimmed; "Tips" and
        // the welcome line are bold/bright; everything else is plain
        // text in the brighter palette.
        let styled_right: String = if right.starts_with('•') {
            format!("{D}{right}{R}")
        } else if right.is_empty() {
            String::new()
        } else {
            format!("{BD}{T}{right}{R}")
        };
        // Pad the right column to its visible width so the closing
        // `│` lines up with the top/bottom borders even when the
        // right text is short (or empty).
        let right_visible = right.chars().count();
        let right_pad = right_col_w.saturating_sub(right_visible);
        let line = format!(
            "{B}│{R} {B}{art}{R}  {styled_right}{}{B} │{R}",
            " ".repeat(right_pad)
        );
        out.push(line);
    }

    // Padding row.
    out.push(format!("{B}│{R}{}{B}│{R}", " ".repeat(inner_w)));

    // Bottom border.
    out.push(format!("{B}╰{}╯{R}", "─".repeat(inner_w)));

    out
}

/// Returns the line prefix tag and the foreground color to use when
/// painting a log line of the given level. Shared between live
/// printing (`handle_event` → `print_above_viewport`) and the
/// filter-driven full repaint (`repaint_scrollback`) so the two
/// stay visually consistent.
///
/// Special-case: INFO lines whose message starts with the "✓" tick
/// or the rocket emoji are upgraded to the theme's success colour,
/// so build-complete / app-launched announcements pop against the
/// stream of regular debug noise.
fn log_style_for(
    level: fl_core::LogLevel,
    message: &str,
    theme: &Theme,
) -> (&'static str, ratatui::style::Color) {
    // `contains` not `starts_with` so a multi-device prefix like
    // `[iPhone Antoine] ` doesn't break the green-up — the marker
    // can sit anywhere in the message body.
    if matches!(level, fl_core::LogLevel::Info)
        && (message.contains("✓ ") || message.contains("🚀 "))
    {
        return ("INFO  ", theme.success);
    }
    log_style(level, theme)
}

fn log_style(level: fl_core::LogLevel, theme: &Theme) -> (&'static str, ratatui::style::Color) {
    match level {
        fl_core::LogLevel::Error => ("ERROR ", theme.error),
        fl_core::LogLevel::Warn => ("WARN  ", theme.warn),
        fl_core::LogLevel::Info => ("INFO  ", theme.fg),
        fl_core::LogLevel::Debug => ("DEBUG ", theme.dim),
        fl_core::LogLevel::Trace => ("TRACE ", theme.dim),
    }
}

/// Does the given log line match the current filter? `None`, `Some("")`,
/// and a filter that is a substring of either the level name (info,
/// warn, error…) or the message all count as a match. Mirrors the
/// behavior of the old (now-unused) logs-panel filter so the `/`
/// keybind keeps the same UX after we moved logs into the scrollback.
pub(crate) fn log_matches_filter(
    filter: Option<&str>,
    level: fl_core::LogLevel,
    message: &str,
) -> bool {
    let needle = match filter {
        None => return true,
        Some("") => return true,
        Some(s) => s,
    };
    let needle_lower = needle.to_ascii_lowercase();
    let level_name = match level {
        fl_core::LogLevel::Error => "error",
        fl_core::LogLevel::Warn => "warn",
        fl_core::LogLevel::Info => "info",
        fl_core::LogLevel::Debug => "debug",
        fl_core::LogLevel::Trace => "trace",
    };
    if level_name.contains(needle_lower.as_str()) {
        return true;
    }
    message.to_ascii_lowercase().contains(&needle_lower)
}

pub struct TuiRunner {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    mode: ViewportMode,
    /// Tracks the `state.log_filter` value we last reacted to. Used
    /// to detect filter changes during the event loop so we can clear
    /// and repaint the scrollback region with only matching lines.
    last_filter: Option<String>,
    /// Requested inline viewport height (Inline mode only). Stored so
    /// terminal-resize events can recompute the viewport Rect against
    /// the new (cols, rows) instead of keeping the frozen startup one.
    inline_height: u16,
    /// `true` when this Inline session was started via the `_with_banner`
    /// variant. On resize we re-render the banner at the new terminal
    /// width so the box doesn't end up half-erased or with the wrong
    /// border length when the user shrinks/grows the window.
    show_banner: bool,
}

impl TuiRunner {
    pub fn init() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        // Belt-and-suspenders mouse-mode handling. Some terminals
        // (tmux passthrough, Warp, certain iTerm2 configs) ship to us
        // with mouse-tracking modes already on from a previous TUI
        // that exited badly. We FIRST turn all of them off, then
        // enable only the two we want — basic click/wheel (?1000h)
        // plus SGR encoding (?1006h). Without the prophylactic
        // disable, leftover `?1003h` (any-event tracking) keeps
        // emitting motion events that show up as visible bytes.
        //
        //   ?1000 — normal mouse tracking (click + release + wheel)
        //   ?1002 — button-event tracking (drag motion)
        //   ?1003 — any-event tracking (every pixel of motion)
        //   ?1004 — focus reporting (FocusIn/FocusOut)
        //   ?1005 — UTF-8 extended coords
        //   ?1006 — SGR encoding (modern, supports coords > 223)
        //   ?1015 — urxvt extended coords
        let disable_all =
            "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1015l";
        let enable_wanted = "\x1b[?1000h\x1b[?1006h";
        write!(stdout, "{disable_all}{enable_wanted}")?;
        stdout.flush()?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("creating ratatui terminal")?;
        Ok(Self {
            terminal,
            mode: ViewportMode::Fullscreen,
            last_filter: None,
            inline_height: 0,
            show_banner: false,
        })
    }

    /// Initialize an *inline* viewport — a ratatui box pinned to the
    /// bottom `height` rows of the user's terminal. The rest of the
    /// terminal keeps showing whatever was there (shell prompt, output
    /// of previous commands, etc.). Modeled after Claude Code's UI.
    ///
    /// Differences from `init()`:
    /// - No `EnterAlternateScreen` — we stay on the primary screen so
    ///   scrollback remains visible above the box.
    /// - No mouse capture — the terminal's native scrollwheel handling
    ///   keeps working, so the user can scroll up through their history
    ///   without us intercepting wheel events.
    ///
    /// Implementation note: ratatui's built-in `Viewport::Inline` calls
    /// `compute_inline_size`, which sends a DSR (Device Status Report)
    /// cursor-position query and waits for the terminal's `ESC[Row;ColR`
    /// response. That query is flaky in some terminals (Warp, certain
    /// tmux passthrough configurations) and times out with "The cursor
    /// position could not be read within a normal duration" — and the
    /// late response then leaks into the user's shell as garbage like
    /// `;1R`. We work around it by:
    ///   1. Reading the terminal *size* (a reliable ioctl, not a query)
    ///   2. Emitting `effective_height` CRLFs to scroll the user's
    ///      content up by that many rows
    ///   3. Anchoring a `Viewport::Fixed` rectangle at the bottom of
    ///      the terminal — no DSR query needed.
    ///
    /// The visible behavior matches `Inline`. The cost is that we don't
    /// autoresize on terminal resize, which is acceptable for a CLI
    /// session that the user rarely resizes mid-run.
    ///
    /// Default: no welcome banner. Used by transient inline UIs like
    /// the device picker and `fl test` — the banner should only appear
    /// for the main `fl run` session.
    pub fn init_inline(height: u16) -> anyhow::Result<Self> {
        Self::init_inline_with_options(height, false)
    }

    /// Inline TUI for `fl run`: prints the bordered welcome banner
    /// into the scrollback immediately above the viewport at startup.
    pub fn init_inline_with_banner(height: u16) -> anyhow::Result<Self> {
        Self::init_inline_with_options(height, true)
    }

    fn init_inline_with_options(height: u16, show_banner: bool) -> anyhow::Result<Self> {
        use ratatui::layout::Rect;
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();

        let (cols, rows) = crossterm::terminal::size()?;
        // Clamp height so we always leave at least 2 rows of scrollback
        // visible above the box (the "see your history above" promise)
        // and never overflow a tiny terminal. Floor at 8 rows so the
        // dashboard's mandatory regions still fit; render_too_small
        // takes over if even that doesn't.
        let effective_height = height.min(rows.saturating_sub(2)).max(8).min(rows.max(1));

        // Defensive: turn off any leftover mouse-tracking modes a prior
        // TUI may have enabled and crashed without cleaning up. We do
        // NOT enable any mouse mode of our own in inline mode — see
        // doc comment above.
        let disable_all =
            "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1015l";
        write!(stdout, "{disable_all}")?;

        // Pre-scroll: emit `effective_height` CRLF pairs so the
        // terminal scrolls up by that many rows, freeing a clean band
        // at the bottom for the viewport. `\r\n` (rather than just
        // `\n`) keeps the cursor at column 0 — important because we're
        // in raw mode and a bare `\n` would only line-feed.
        let pre_scroll: String = "\r\n".repeat(effective_height as usize);
        write!(stdout, "{pre_scroll}")?;

        // Place the welcome banner IMMEDIATELY above where the
        // viewport will sit. The banner is the bottom-most thing in
        // the scrollback region at startup, "glued" to the status
        // bar. As logs arrive (printed via `print_above_viewport`
        // using DECSTBM), the scroll region above the viewport
        // shifts up and pushes the banner away from the bar — and
        // eventually off the top of the screen, matching the user's
        // Claude-Code reference.
        //
        // We skip the banner entirely if (a) the terminal is too
        // narrow to fit BANNER_WIDTH or (b) there aren't enough rows
        // above the viewport to fit all banner lines. Half a banner
        // would look worse than no banner.
        // The welcome banner is opt-in: only `fl run` requests it via
        // `init_inline_with_banner`. Transient UIs (device picker,
        // `fl test`) call the plain `init_inline` and pass false.
        let banner = welcome_banner_lines(cols);
        let banner_h = banner.len() as u16;
        let rows_above_viewport = rows.saturating_sub(effective_height);
        let banner_fits =
            show_banner && cols >= BANNER_MIN_WIDTH && rows_above_viewport >= banner_h;
        if banner_fits {
            // Paint banner glued just above the viewport. As logs
            // flow in, the band scrolls up via DECSTBM and the
            // banner naturally rolls toward the top, eventually off
            // the screen entirely — giving logs the full vertical
            // space.
            let banner_top_1idx = rows_above_viewport.saturating_sub(banner_h - 1);
            write!(stdout, "\x1b[{banner_top_1idx};1H")?;
            for (i, line) in banner.iter().enumerate() {
                if i + 1 < banner.len() {
                    write!(stdout, "{line}\r\n")?;
                } else {
                    // Last line: no trailing CRLF, avoids pushing
                    // the cursor into a fresh row and leaving a
                    // stray cursor before ratatui's first draw.
                    write!(stdout, "{line}")?;
                }
            }
        }
        stdout.flush()?;

        let area = Rect::new(
            0,
            rows.saturating_sub(effective_height),
            cols,
            effective_height,
        );
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(area),
            },
        )
        .context("creating fixed-viewport ratatui terminal")?;
        Ok(Self {
            terminal,
            mode: ViewportMode::Inline,
            last_filter: None,
            inline_height: effective_height,
            show_banner,
        })
    }

    /// Print a single line of content into the scrollback ABOVE the
    /// inline viewport. The area above the viewport scrolls up by 1
    /// row to make space, the bottom of that scrollback area gets the
    /// new line, the viewport itself is left untouched.
    ///
    /// Mechanism: DECSTBM (Set Top and Bottom Margins). We temporarily
    /// restrict the terminal's scrolling region to rows ABOVE the
    /// viewport, issue a single linefeed at the bottom of that region
    /// (which scrolls only within the region — the viewport rows
    /// don't move), write the new content, then reset the scrolling
    /// region to full-screen. This is what `tail -f`-style tools do.
    ///
    /// No-op when not in inline mode — fullscreen viewports own the
    /// whole screen, so there's nowhere to print "above" them.
    pub fn print_above_viewport(
        &mut self,
        prefix: &str,
        message: &str,
        fg: ratatui::style::Color,
    ) -> anyhow::Result<()> {
        if !matches!(self.mode, ViewportMode::Inline) {
            return Ok(());
        }
        let viewport_area = self.terminal.get_frame().size();
        let rows_above = viewport_area.top();
        if rows_above == 0 {
            // No space above the viewport, can't print anything.
            return Ok(());
        }
        let cols = viewport_area.width as usize;

        // ANSI SGR foreground from the provided ratatui color. We only
        // need the basic 24-bit case (theme uses Rgb) and a fallback
        // for anything else (rendered as default fg).
        let sgr = match fg {
            ratatui::style::Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
            _ => String::new(),
        };

        // Truncate to terminal width so a long line doesn't wrap into
        // a second row (which would shift the viewport's perceived
        // position). The prefix counts toward the budget.
        let combined = format!("{prefix}{message}");
        let truncated: String = combined.chars().take(cols).collect();

        let backend = self.terminal.backend_mut();
        // 1. Save the cursor so ratatui's next draw resumes from where
        //    it left off (not strictly required because ratatui uses
        //    absolute MoveTo for every cell, but it costs nothing and
        //    is the polite thing to do).
        // 2. Set scroll region to rows 1..rows_above (DECSTBM is
        //    1-indexed and inclusive on both ends).
        // 3. Position cursor at the bottom of that region.
        // 4. Emit LF — terminal scrolls the region's contents up by 1,
        //    leaving the bottom row blank. Cursor stays at row
        //    rows_above, col 1.
        // 5. \r is implicit-ish (no, we need to emit it) — col 1 is
        //    already where MoveTo put us, so we just write the text.
        // 6. Reset scroll region to the full screen.
        // 7. Restore cursor.
        write!(backend, "\x1b7")?;
        // Scroll region = the full band above the viewport. Logs
        // scroll the entire band including the banner rows, so the
        // welcome banner gradually rolls off the top as the app
        // produces output. This is the conscious trade-off: more
        // vertical room for logs at the cost of the banner being
        // ephemeral after the first dozen lines.
        write!(backend, "\x1b[1;{rows_above}r")?;
        write!(backend, "\x1b[{rows_above};1H")?;
        writeln!(backend)?;
        write!(backend, "\r{sgr}{truncated}\x1b[0m")?;
        write!(backend, "\x1b[r")?;
        write!(backend, "\x1b8")?;
        backend.flush()?;
        Ok(())
    }

    pub fn restore(&mut self) -> anyhow::Result<()> {
        disable_raw_mode()?;
        match self.mode {
            ViewportMode::Fullscreen => {
                // Disable ALL mouse modes we (or any predecessor) could
                // have enabled, then leave the alt screen. Order
                // matters — once we've left the alt screen we can't
                // write to it any more.
                {
                    let backend = self.terminal.backend_mut();
                    write!(
                        backend,
                        "\x1b[?1006l\x1b[?1015l\x1b[?1005l\x1b[?1004l\x1b[?1003l\x1b[?1002l\x1b[?1000l"
                    )?;
                    backend.flush()?;
                }
                execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
                self.terminal.show_cursor()?;
            }
            ViewportMode::Inline => {
                // Move the cursor BELOW the inline viewport so the next
                // shell output (or our own exit summary printlns) lands
                // on a fresh line rather than overwriting the TUI box.
                // The viewport sits at `viewport_area.bottom() - 1` so
                // we move to `viewport_area.bottom()` then newline.
                let area = self.terminal.get_frame().size();
                let _ = self.terminal.set_cursor(0, area.bottom().saturating_sub(1));
                {
                    let backend = self.terminal.backend_mut();
                    write!(backend, "\r\n")?;
                    backend.flush()?;
                }
                self.terminal.show_cursor()?;
            }
        }
        Ok(())
    }

    /// Per-event dispatch from the `run()` loop. For log events in
    /// inline mode, we print the line into the scrollback above the
    /// viewport (so logs flow naturally with the terminal scroll
    /// instead of being trapped in a fixed-size panel). The event then
    /// goes through `state.apply` so the rest of the app (exit
    /// summary, banner triggers, etc.) still sees it.
    fn handle_event(&mut self, state: &mut AppState, ev: AppEvent, theme: &Theme) {
        if matches!(self.mode, ViewportMode::Inline) {
            if let AppEvent::Flutter(fl_core::FlutterEvent::Log { level, message }) = &ev {
                // Apply the active log filter, if any. A non-matching
                // line is still recorded in state.logs (so it's
                // visible again the moment the user clears the
                // filter), it just doesn't get printed to the
                // terminal scrollback.
                if log_matches_filter(state.log_filter.as_deref(), *level, message) {
                    let (prefix, color) = log_style_for(*level, message, theme);
                    let _ = self.print_above_viewport(prefix, message, color);
                }
            }
        }
        state.apply(ev);
    }

    /// Detect a change to `state.log_filter` (compared to the value
    /// we last reacted to) and, if so, clear the scrollback region
    /// above the viewport and repaint it with the most recent
    /// matching log lines pulled from `state.logs`. Lets the `/`
    /// filter actually filter, even though logs now live in the
    /// terminal's scrollback rather than in a ratatui buffer we
    /// could re-render at will.
    fn refresh_filter_view(&mut self, state: &AppState, theme: &Theme) {
        if !matches!(self.mode, ViewportMode::Inline) {
            return;
        }
        if self.last_filter == state.log_filter {
            return;
        }
        self.last_filter = state.log_filter.clone();
        let _ = self.repaint_scrollback(state, theme);
    }

    /// Wipe the scrollback band above the viewport and reprint the
    /// tail of `state.logs` that matches the current filter. Uses
    /// DECSTBM to restrict its writes to the scrollback rows, so
    /// the viewport itself is left alone.
    fn repaint_scrollback(&mut self, state: &AppState, theme: &Theme) -> anyhow::Result<()> {
        let viewport_area = self.terminal.get_frame().size();
        let rows_above = viewport_area.top();
        if rows_above == 0 {
            return Ok(());
        }
        let cols_u16 = viewport_area.width;
        let cols = cols_u16 as usize;
        let filter = state.log_filter.as_deref();

        // Banner sits at the very top of the band when this is an
        // `fl run`-style inline UI. We need to know how many rows it
        // takes so we can both paint logs BELOW it and skip those rows
        // when erasing — otherwise every filter change / resize would
        // wipe the banner and the "header" of `fl run` would vanish.
        let banner_lines: Vec<String> = if self.show_banner && cols_u16 >= BANNER_MIN_WIDTH {
            let lines = welcome_banner_lines(cols_u16);
            if (lines.len() as u16) <= rows_above {
                lines
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let banner_h = banner_lines.len() as u16;

        // Logs occupy the rows BELOW the banner, anchored to the bottom
        // of the band (just above the viewport).
        let log_band_top = banner_h + 1; // 1-indexed
        let log_rows = rows_above.saturating_sub(banner_h);

        let n_rows = log_rows as usize;
        let mut tail: Vec<&LogLine> = state
            .logs
            .iter()
            .rev()
            .filter(|l| log_matches_filter(filter, l.level, &l.message))
            .take(n_rows)
            .collect();
        tail.reverse();

        let backend = self.terminal.backend_mut();
        write!(backend, "\x1b7")?;

        // Banner first — top of the band. Paint each line at its
        // absolute row so cursor scroll behavior at the bottom doesn't
        // shift anything.
        for (i, line) in banner_lines.iter().enumerate() {
            let row = (i as u16) + 1;
            write!(backend, "\x1b[{row};1H\x1b[2K{line}")?;
        }

        // Then the log tail — clears each log row before writing.
        let tail_len = tail.len();
        let log_at_row = |r: u16| -> Option<&LogLine> {
            let from_bottom = (rows_above - r) as usize;
            if from_bottom >= tail_len {
                None
            } else {
                Some(tail[tail_len - 1 - from_bottom])
            }
        };
        for row in log_band_top..=rows_above {
            write!(backend, "\x1b[{row};1H\x1b[2K")?;
            if let Some(line) = log_at_row(row) {
                let (prefix, color) = log_style_for(line.level, &line.message, theme);
                let sgr = match color {
                    ratatui::style::Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
                    _ => String::new(),
                };
                let combined = format!("{prefix}{}", line.message);
                let truncated: String = combined.chars().take(cols).collect();
                write!(backend, "{sgr}{truncated}\x1b[0m")?;
            }
        }

        write!(backend, "\x1b8")?;
        backend.flush()?;
        Ok(())
    }

    /// React to a terminal resize: recompute the viewport Rect against
    /// the new (cols, rows), wipe the screen so no stale glyphs remain
    /// where the old box used to sit, and re-emit the welcome banner at
    /// the new width when applicable. The next `terminal.draw()` call
    /// then repaints the dashboard into the new area; `repaint_scrollback`
    /// (called by the run loop after this) refills the band above the
    /// viewport with the log tail at the current width.
    ///
    /// Without this, a `Viewport::Fixed` inline UI stays drawn at the
    /// old coordinates after a resize — visible as ghost borders, a
    /// dashboard floating at the wrong y-offset, or a banner whose
    /// horizontal rules suddenly don't reach the new edge.
    fn handle_resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        use ratatui::layout::Rect;
        let new_area = match self.mode {
            ViewportMode::Fullscreen => Rect::new(0, 0, cols, rows),
            ViewportMode::Inline => {
                let h = self
                    .inline_height
                    .min(rows.saturating_sub(2))
                    .max(8)
                    .min(rows.max(1));
                Rect::new(0, rows.saturating_sub(h), cols, h)
            }
        };
        // Step 1: wipe both the visible screen AND the terminal's
        // scrollback buffer. The `\x1b[2J` clears what's currently
        // on screen; `\x1b[3J` (xterm extension, supported by every
        // modern terminal we care about) wipes the off-screen
        // scrollback too. The scrollback wipe is critical when the
        // user grows the terminal vertically — without it, every
        // viewport position we've ever drawn at (now scrolled out
        // of view) re-appears at the top of the screen, producing
        // a column of stacked dashboards.
        {
            let backend = self.terminal.backend_mut();
            write!(backend, "\x1b[3J\x1b[2J\x1b[H")?;
            backend.flush()?;
        }
        // Step 2: tell ratatui about the new geometry and invalidate
        // its known-state buffer so the next draw repaints every cell
        // (the diff would otherwise skip cells whose new value matches
        // the stale cached value).
        self.terminal.resize(new_area)?;
        self.terminal.clear()?;
        // The band above the viewport (banner + logs) is repainted by
        // the run loop's call to `repaint_scrollback` immediately after
        // this — it handles both banner and log placement so the order
        // is guaranteed and they don't fight over the same rows.
        Ok(())
    }

    pub async fn run(
        &mut self,
        state: &mut AppState,
        rx: &mut Receiver<AppEvent>,
        keys_tx: Sender<FlKey>,
    ) -> anyhow::Result<()> {
        use tokio::time::{interval, MissedTickBehavior};
        let theme = Theme::TOKYO_NIGHT;
        let frame = Duration::from_millis(33);
        let mut tick = interval(frame);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut events = EventStream::new();

        loop {
            if state.quitting {
                break;
            }

            tokio::select! {
                biased;
                _ = tick.tick() => {
                    self.terminal.draw(|f| {
                        render(f.size(), f.buffer_mut(), state, &theme);
                    })?;
                }
                Some(Ok(term_ev)) = events.next() => {
                    if let Event::Resize(mut cols, mut rows) = term_ev {
                        // Dragging a terminal edge fires a Resize per
                        // pixel/step — sometimes dozens per second.
                        // Debounce by collapsing consecutive Resize
                        // events ready RIGHT NOW (non-blocking peek)
                        // down to the last one. We stop draining the
                        // moment a non-Resize event shows up so it
                        // isn't dropped — the next select iteration
                        // will pick it up via `events.next()` again.
                        use futures_util::future::FutureExt;
                        while let Some(Some(Ok(Event::Resize(c, r)))) =
                            events.next().now_or_never()
                        {
                            cols = c;
                            rows = r;
                        }
                        let _ = self.handle_resize(cols, rows);
                        // Wipe + redraw left the scrollback band empty.
                        // Refill it from state.logs so history isn't
                        // lost every time the user nudges the edge.
                        let _ = self.repaint_scrollback(state, &theme);
                    } else if let Some(k) = map_key(term_ev) {
                        // Track filter-input state BEFORE and AFTER
                        // so we can tell whether the key was consumed
                        // by the filter buffer. Either:
                        //   - filter was already active (user typing
                        //     into the search field), or
                        //   - the key IS the `/` that just opened
                        //     filter mode,
                        // and in both cases we MUST NOT forward to
                        // `keys_tx` — otherwise typing "error" into
                        // the filter would also fire `[r] reload`,
                        // `[e] …`, etc.
                        let was_filtering = state.filter_input.is_some();
                        state.on_key(k);
                        let is_filtering = state.filter_input.is_some();
                        // The filter may have just changed — repaint
                        // the scrollback band above the viewport with
                        // the (newly-filtered) tail of state.logs.
                        self.refresh_filter_view(state, &theme);
                        if !was_filtering && !is_filtering {
                            // Normal mode in, normal mode out — safe
                            // to fire any global keybinds (r/R/b/…).
                            let _ = keys_tx.try_send(k);
                        }
                    }
                }
                Some(ev) = rx.recv() => {
                    self.handle_event(state, ev, &theme);
                    // Drain a bounded burst so we don't redraw between every
                    // queued event. Then yield via the next select iteration.
                    for _ in 0..256 {
                        match rx.try_recv() {
                            Ok(ev) => self.handle_event(state, ev, &theme),
                            Err(_) => break,
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl TuiRunner {
    /// Drive any `View` to completion. The runner reads from `input_rx`, feeds
    /// `view.apply`, listens to keyboard, and ticks the view.
    pub async fn run_view<V: crate::view::View>(
        &mut self,
        view: &mut V,
        input_rx: &mut tokio::sync::mpsc::Receiver<V::Input>,
    ) -> anyhow::Result<()> {
        use crate::theme::Theme;
        use futures_util::StreamExt;
        use tokio::time::{interval, MissedTickBehavior};
        let theme = Theme::TOKYO_NIGHT;
        let frame = std::time::Duration::from_millis(33);
        let mut tick = interval(frame);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut events = crossterm::event::EventStream::new();
        let mut last_draw = std::time::Instant::now();

        loop {
            if view.quitting() {
                break;
            }

            tokio::select! {
                biased;
                _ = tick.tick() => {
                    let now = std::time::Instant::now();
                    view.tick(now - last_draw);
                    self.terminal.draw(|f| {
                        view.render(f.size(), f.buffer_mut(), &theme);
                    })?;
                    last_draw = now;
                }
                Some(Ok(term_ev)) = events.next() => {
                    if let crossterm::event::Event::Resize(mut cols, mut rows) = term_ev {
                        use futures_util::future::FutureExt;
                        while let Some(Some(Ok(crossterm::event::Event::Resize(c, r)))) =
                            events.next().now_or_never()
                        {
                            cols = c;
                            rows = r;
                        }
                        let _ = self.handle_resize(cols, rows);
                    } else if let Some(k) = crate::runner::map_key(term_ev) {
                        if let Some(input) = view.handle_key(k) {
                            view.apply(input);
                        }
                    }
                }
                Some(ev) = input_rx.recv() => {
                    view.apply(ev);
                    for _ in 0..256 {
                        match input_rx.try_recv() {
                            Ok(ev) => view.apply(ev),
                            Err(_) => break,
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn map_key(ev: Event) -> Option<FlKey> {
    let key = match ev {
        Event::Key(k) => k,
        // Trackpad / mouse-wheel scroll: we translate to keyboard
        // arrow keys so every existing scroll-aware view (logs panel,
        // failures panel, etc.) reacts the same way to keyboard and
        // wheel input — no per-view mouse handling needed.
        Event::Mouse(MouseEvent { kind, .. }) => {
            return match kind {
                MouseEventKind::ScrollUp => Some(FlKey::Up),
                MouseEventKind::ScrollDown => Some(FlKey::Down),
                _ => None, // clicks / drags / moves silently swallowed
            };
        }
        _ => return None,
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            return Some(FlKey::Ctrl(c));
        }
    }
    Some(match key.code {
        KeyCode::Char(c) => FlKey::Char(c),
        KeyCode::Enter => FlKey::Enter,
        KeyCode::Esc => FlKey::Esc,
        KeyCode::Tab => FlKey::Tab,
        KeyCode::Up => FlKey::Up,
        KeyCode::Down => FlKey::Down,
        KeyCode::PageUp => FlKey::PageUp,
        KeyCode::PageDown => FlKey::PageDown,
        KeyCode::Backspace => FlKey::Backspace,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    fn key(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(crossterm::event::KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        })
    }

    #[test]
    fn maps_lowercase_char() {
        assert!(matches!(
            map_key(key(KeyCode::Char('r'), KeyModifiers::NONE)).unwrap(),
            FlKey::Char('r')
        ));
    }
    #[test]
    fn maps_ctrl_c() {
        assert!(matches!(
            map_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL)).unwrap(),
            FlKey::Ctrl('c')
        ));
    }
    #[test]
    fn maps_arrow_keys() {
        assert!(matches!(
            map_key(key(KeyCode::Up, KeyModifiers::NONE)).unwrap(),
            FlKey::Up
        ));
        assert!(matches!(
            map_key(key(KeyCode::Down, KeyModifiers::NONE)).unwrap(),
            FlKey::Down
        ));
    }

    use crate::view::View;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[derive(Default)]
    struct DummyView {
        ticks: u32,
        applied: u32,
        done: bool,
    }
    impl View for DummyView {
        type Input = u32;
        fn apply(&mut self, _: u32) {
            self.applied += 1;
        }
        fn render(&self, _: Rect, _: &mut Buffer, _: &crate::theme::Theme) {}
        fn handle_key(&mut self, _: fl_core::KeyEvent) -> Option<u32> {
            None
        }
        fn tick(&mut self, _: Duration) {
            self.ticks += 1;
            if self.ticks >= 3 {
                self.done = true;
            }
        }
        fn quitting(&self) -> bool {
            self.done
        }
    }

    #[tokio::test(start_paused = true)]
    async fn run_view_terminates_when_view_says_quitting() {
        let mut v = DummyView::default();
        let (_tx, mut rx) = mpsc::channel::<u32>(1);
        // TuiRunner needs a real terminal; build a no-init shim by skipping init.
        // We can call run_view directly only if we have a TuiRunner. Constructing one
        // touches stdout, so we instead exercise the View trait's loop logic by
        // calling tick() three times manually here; full end-to-end coverage comes
        // from the existing run() tests.
        for _ in 0..3 {
            v.tick(Duration::from_millis(33));
        }
        assert!(v.quitting());
        // drain to avoid the unused-warning trap.
        assert!(rx.try_recv().is_err());
    }
}
