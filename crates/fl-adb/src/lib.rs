//! ADB integration: device discovery, pre-pairing (USB→WiFi), and live watching.

pub mod mdns;
pub mod pair;
pub mod parse;
pub mod reconnect;
pub mod runner;
pub mod watcher;

pub use mdns::{matches_device, pick_ipv4, SERVICE_TYPES};
pub use pair::{pre_pair_wifi, WifiTarget};
pub use parse::{parse_devices_l, parse_wlan_ip};
pub use reconnect::{backoff_delay, spawn, transition, Input, ManagerHandle, ManagerSetup, Output, State};
pub use runner::{CommandOutput, CommandRunner, MockRunner, TokioRunner};
pub use watcher::{diff_devices, parse_track_payload, track_devices};
