//! Clap definitions for the `fl` binary.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "fl", version, about = "A modern Flutter CLI with seamless USB→WiFi hot reload")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// List attached devices with status, IP, battery, OS version.
    Devices,
    /// Run a Flutter app with the `fl` dashboard. Auto-pairs USB→WiFi.
    Run {
        /// Path to the Flutter project (defaults to cwd).
        #[arg(short, long)]
        project: Option<PathBuf>,
        /// Force a specific device serial (skip auto-pair).
        #[arg(short, long)]
        device: Option<String>,
        /// Disable USB→WiFi pre-pairing.
        #[arg(long)]
        no_wifi: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_devices_subcommand() {
        let c = Cli::parse_from(["fl", "devices"]);
        assert!(matches!(c.cmd, Cmd::Devices));
    }

    #[test]
    fn parses_run_with_options() {
        let c = Cli::parse_from(["fl", "run", "--device", "1.2.3.4:5555", "--no-wifi"]);
        match c.cmd {
            Cmd::Run { device, no_wifi, .. } => {
                assert_eq!(device.as_deref(), Some("1.2.3.4:5555"));
                assert!(no_wifi);
            }
            _ => panic!(),
        }
    }
}
