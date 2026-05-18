//! Apple device discovery via `xcrun devicectl` and `xcrun simctl`.

pub mod xcrun;

pub use xcrun::Xcrun;
