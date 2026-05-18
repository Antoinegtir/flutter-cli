//! ADB integration: device discovery, pre-pairing (USB→WiFi), and live watching.

pub mod runner;

pub use runner::{CommandOutput, CommandRunner, MockRunner, TokioRunner};
