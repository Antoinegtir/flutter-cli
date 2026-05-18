//! Apple device discovery via `xcrun devicectl` and `xcrun simctl`.

pub mod parse;
pub mod xcrun;

pub use parse::{parse_devicectl_json, parse_simctl_json};
pub use xcrun::Xcrun;
