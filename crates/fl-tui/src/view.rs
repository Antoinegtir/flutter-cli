//! Generic `View` trait so `TuiRunner` can host multiple command-specific UIs.

use crate::theme::Theme;
use fl_core::KeyEvent as FlKey;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::time::Duration;

pub trait View: Send + 'static {
    /// Event type the view consumes. The runner pushes these via `apply`.
    type Input: Send + 'static;

    /// Apply an event from the producer side (e.g. parsed daemon output).
    fn apply(&mut self, input: Self::Input);

    /// Draw the current state into `buf`.
    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme);

    /// Translate a terminal key into a view-specific `Input` (or `None`).
    /// The runner sends the returned `Input` back through `apply`, so the same
    /// state-mutation code path handles both external and key-derived events.
    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input>;

    /// Called every 33 ms with the elapsed time since the last tick — used for
    /// animations and banner expiry.
    fn tick(&mut self, dt: Duration);

    /// Returns `true` to ask the runner to break out of the loop.
    fn quitting(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;
    use fl_core::{AppEvent, FlutterEvent, LogLevel};
    use ratatui::buffer::Buffer;
    use std::time::Duration;

    #[test]
    fn appstate_view_apply_and_render_compile_and_run() {
        let mut s = AppState::new("app".into(), "debug".into());
        <AppState as View>::apply(
            &mut s,
            AppEvent::Flutter(FlutterEvent::Log {
                level: LogLevel::Info,
                message: "hi".into(),
            }),
        );
        let theme = crate::theme::Theme::TOKYO_NIGHT;
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        <AppState as View>::render(&s, Rect::new(0, 0, 80, 24), &mut buf, &theme);
        assert!(!<AppState as View>::quitting(&s));
    }

    #[test]
    fn appstate_view_handle_key_quit_sets_flag() {
        let mut s = AppState::new("app".into(), "debug".into());
        let _ = <AppState as View>::handle_key(&mut s, fl_core::KeyEvent::Char('q'));
        assert!(<AppState as View>::quitting(&s));
    }

    // Note: `Duration` is imported but not used directly; the import keeps consistency
    // with future tests that exercise `tick()`. Allow unused-import locally if needed.
    #[allow(dead_code)]
    fn _unused_duration() -> Duration { Duration::from_millis(0) }
}
