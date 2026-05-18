//! Terminal UI for the `fl` CLI.

pub mod app;
pub mod panels;
pub mod spinner;
pub mod splash;
pub mod theme;

pub use app::{AppState, Banner, BannerKind, LogLine};
pub use spinner::Spinner;
pub use splash::Splash;
pub use theme::Theme;
