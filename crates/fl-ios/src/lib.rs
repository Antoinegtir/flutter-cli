//! Apple device discovery via `xcrun devicectl` and `xcrun simctl`.

pub mod parse;
pub mod watcher;
pub mod xcrun;

pub use parse::{parse_devicectl_json, parse_simctl_json};
pub use watcher::{diff_devices, list_apple_devices, watch_apple_devices};
pub use xcrun::Xcrun;
