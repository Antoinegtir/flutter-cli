//! Braille-pattern spinner. One frame every 80 ms.

use std::time::Duration;

const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const FRAME_MS: u64 = 80;

#[derive(Debug, Clone, Copy, Default)]
pub struct Spinner {
    pub elapsed_ms: u64,
}

impl Spinner {
    pub fn tick(&mut self, dt: Duration) {
        self.elapsed_ms = self.elapsed_ms.saturating_add(dt.as_millis() as u64);
    }
    pub fn frame(&self) -> char {
        let idx = (self.elapsed_ms / FRAME_MS) as usize % FRAMES.len();
        FRAMES[idx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_advances_every_80ms() {
        let mut s = Spinner::default();
        let f0 = s.frame();
        s.tick(Duration::from_millis(80));
        let f1 = s.frame();
        assert_ne!(f0, f1);
    }

    #[test]
    fn frame_cycles_through_all_then_repeats() {
        let mut s = Spinner::default();
        let first = s.frame();
        s.tick(Duration::from_millis(80 * FRAMES.len() as u64));
        assert_eq!(s.frame(), first);
    }
}
