//! Terminal UI for the `fl` CLI.

pub mod app;
pub mod panels;
pub mod render;
pub mod runner;
pub mod spinner;
pub mod theme;
pub mod view;
pub mod views;

pub use app::{AppState, Banner, BannerKind, LogLine};
pub use render::render;
pub use runner::{map_key, TuiRunner};
pub use spinner::Spinner;
pub use theme::Theme;
pub use view::View;
pub use views::build_view::BuildView;
pub use views::device_picker::{DevicePickerInput, DevicePickerOutcome, DevicePickerView};
pub use views::pub_view::{PubMode, PubView};
pub use views::test_view::TestView;
