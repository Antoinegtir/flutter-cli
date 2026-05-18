//! ADB integration: device discovery, pre-pairing (USB→WiFi), and live watching.

pub mod pair;
pub mod parse;
pub mod runner;

pub use pair::{pre_pair_wifi, WifiTarget};
pub use parse::{parse_devices_l, parse_wlan_ip};
pub use runner::{CommandOutput, CommandRunner, MockRunner, TokioRunner};
