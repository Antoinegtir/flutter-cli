//! Drives the TUI: render loop on a tick, drain `AppEvent` channel, handle key input.

use crate::app::AppState;
use crate::render::render;
use crate::splash::Splash;
use crate::theme::Theme;
use anyhow::Context;
use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use fl_core::{AppEvent, KeyEvent as FlKey};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{Receiver, Sender};

pub struct TuiRunner {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TuiRunner {
    pub fn init() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("creating ratatui terminal")?;
        Ok(Self { terminal })
    }

    pub fn restore(&mut self) -> anyhow::Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }

    pub async fn run(
        &mut self,
        state: &mut AppState,
        rx: &mut Receiver<AppEvent>,
        keys_tx: Sender<FlKey>,
    ) -> anyhow::Result<()> {
        let theme = Theme::TOKYO_NIGHT;
        let mut splash = Splash::new(theme);
        let mut last_tick = Instant::now();
        let tick = Duration::from_millis(33);
        let mut events = EventStream::new();

        loop {
            if state.quitting {
                break;
            }
            let now = Instant::now();
            let dt = now - last_tick;
            last_tick = now;
            if !splash.done() {
                splash.tick(dt);
                self.terminal.draw(|f| {
                    let area = f.size();
                    f.render_widget(&splash, area);
                })?;
            } else {
                self.terminal.draw(|f| {
                    render(f.size(), f.buffer_mut(), state, &theme);
                })?;
            }

            tokio::select! {
                Some(ev) = rx.recv() => {
                    state.apply(ev);
                }
                Some(Ok(term_ev)) = events.next() => {
                    if let Some(k) = map_key(term_ev) {
                        if matches!(k, FlKey::Char('q') | FlKey::Ctrl('c')) {
                            state.quitting = true;
                        }
                        keys_tx.send(k).await.ok();
                    }
                }
                _ = tokio::time::sleep(tick) => {}
            }
        }
        Ok(())
    }
}

pub fn map_key(ev: Event) -> Option<FlKey> {
    let key = match ev {
        Event::Key(k) => k,
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
        assert!(matches!(map_key(key(KeyCode::Char('r'), KeyModifiers::NONE)).unwrap(), FlKey::Char('r')));
    }
    #[test]
    fn maps_ctrl_c() {
        assert!(matches!(map_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL)).unwrap(), FlKey::Ctrl('c')));
    }
    #[test]
    fn maps_arrow_keys() {
        assert!(matches!(map_key(key(KeyCode::Up, KeyModifiers::NONE)).unwrap(), FlKey::Up));
        assert!(matches!(map_key(key(KeyCode::Down, KeyModifiers::NONE)).unwrap(), FlKey::Down));
    }
}
