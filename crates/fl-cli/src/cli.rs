//! Clap definitions for the `fl` binary.

use clap::{Parser, Subcommand};
use fl_core::{BuildMode, BuildTarget};
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
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(short, long)] device: Vec<String>,
        #[arg(long)] all: bool,
        #[arg(long)] no_picker: bool,
        #[arg(long)] no_wifi: bool,
        #[arg(long, value_enum, default_value_t = BuildMode::Debug)] mode: BuildMode,
    },
    /// Build a Flutter app for a given target.
    Build {
        #[arg(value_enum)] target: BuildTarget,
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = BuildMode::Release)] mode: BuildMode,
    },
    /// Run flutter test with a live TUI.
    Test {
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(short, long)] name: Option<String>,
    },
    /// flutter pub subcommands.
    Pub {
        #[command(subcommand)] sub: PubSub,
        #[arg(short, long, global = true)] project: Option<PathBuf>,
    },
    /// flutter doctor with a TUI.
    Doctor,
    /// flutter clean with byte counting.
    Clean {
        #[arg(short, long)] project: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum PubSub {
    Get,
    Upgrade,
    Outdated,
    Deps,
    Add { package: String },
    Remove { package: String },
}

impl PubSub {
    pub fn label(&self) -> &'static str {
        match self {
            PubSub::Get => "get",
            PubSub::Upgrade => "upgrade",
            PubSub::Outdated => "outdated",
            PubSub::Deps => "deps",
            PubSub::Add { .. } => "add",
            PubSub::Remove { .. } => "remove",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_devices_subcommand() {
        let c = Cli::parse_from(["fl", "devices"]);
        assert!(matches!(c.cmd, Cmd::Devices));
    }

    #[test]
    fn parses_run_with_options() {
        let c = Cli::parse_from(["fl", "run", "--device", "1.2.3.4:5555", "--no-wifi"]);
        match c.cmd {
            Cmd::Run { device, no_wifi, mode, all, .. } => {
                assert_eq!(device, vec!["1.2.3.4:5555".to_string()]);
                assert!(no_wifi);
                assert!(!all);
                assert_eq!(mode, BuildMode::Debug);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_run_with_repeated_device() {
        let c = Cli::parse_from(["fl", "run", "--device", "a", "--device", "b"]);
        match c.cmd {
            Cmd::Run { device, .. } => assert_eq!(device, vec!["a".to_string(), "b".to_string()]),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_run_with_all_flag() {
        let c = Cli::parse_from(["fl", "run", "--all"]);
        match c.cmd {
            Cmd::Run { all, .. } => assert!(all),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_run_with_explicit_mode() {
        let c = Cli::parse_from(["fl", "run", "--mode", "release"]);
        match c.cmd {
            Cmd::Run { mode, .. } => assert_eq!(mode, BuildMode::Release),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_run_with_no_picker() {
        let c = Cli::parse_from(["fl", "run", "--no-picker"]);
        match c.cmd {
            Cmd::Run { no_picker, .. } => assert!(no_picker),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_build_apk() {
        let c = Cli::parse_from(["fl", "build", "apk"]);
        match c.cmd {
            Cmd::Build { target, mode, .. } => {
                assert_eq!(target, BuildTarget::Apk);
                assert_eq!(mode, BuildMode::Release);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_pub_add() {
        let c = Cli::parse_from(["fl", "pub", "add", "http"]);
        match c.cmd {
            Cmd::Pub { sub: PubSub::Add { package }, .. } => assert_eq!(package, "http"),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_doctor_and_clean() {
        assert!(matches!(Cli::parse_from(["fl", "doctor"]).cmd, Cmd::Doctor));
        assert!(matches!(Cli::parse_from(["fl", "clean"]).cmd, Cmd::Clean { .. }));
    }
}
