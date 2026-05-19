//! Clap definitions for the `fl` binary.

use clap::{Parser, Subcommand};
use fl_core::BuildMode;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "flutter-cli", version, about = "A modern Flutter CLI with seamless USB→WiFi hot reload")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// List attached devices with status, IP, battery, OS version.
    Devices,
    /// Run a Flutter app with the `fl` dashboard. Auto-pairs USB→WiFi.
    ///
    /// Accepts native `flutter run`-style mode flags (`--release` /
    /// `--profile` / `--debug`). Anything else after a `--` separator
    /// is forwarded verbatim, so the user can pass `--flavor`,
    /// `--dart-define=`, `--target`, etc. without `fl` having to teach
    /// each one explicitly: `fl run --release -- --flavor prod --dart-define=API=https://…`.
    Run {
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(short, long)] device: Vec<String>,
        #[arg(long)] all: bool,
        #[arg(long)] no_picker: bool,
        #[arg(long)] no_wifi: bool,
        /// Skip the dashboard and stream every event to stdout as plain text.
        /// Useful for debugging or when piping into another tool.
        #[arg(long, alias = "logs")] no_tui: bool,
        /// Build in release mode (mirrors `flutter run --release`).
        #[arg(long, conflicts_with_all = ["profile", "debug"])] release: bool,
        /// Build in profile mode (mirrors `flutter run --profile`).
        #[arg(long, conflicts_with = "debug")] profile: bool,
        /// Build in debug mode (the default).
        #[arg(long)] debug: bool,
        /// Pass-through args forwarded verbatim to `flutter run`.
        /// Use a `--` separator: `fl run -- --flavor prod --dart-define=X=Y`.
        #[arg(last = true, allow_hyphen_values = true)] extra: Vec<String>,
    },
    /// Build a Flutter app for a given target.
    ///
    /// `target` accepts any subcommand `flutter build` supports (apk,
    /// appbundle, ios, ipa, macos, web, aar, bundle, …). Omit it to
    /// fall through to `flutter build` and see Flutter's own subcommand
    /// list. Same passthrough convention as `fl run`: `--release` /
    /// `--profile` / `--debug` are recognized; anything else after a
    /// `--` separator is forwarded verbatim.
    Build {
        target: Option<String>,
        #[arg(short, long)] project: Option<PathBuf>,
        #[arg(long, conflicts_with_all = ["profile", "debug"])] release: bool,
        #[arg(long, conflicts_with = "debug")] profile: bool,
        #[arg(long)] debug: bool,
        #[arg(last = true, allow_hyphen_values = true)] extra: Vec<String>,
    },
    /// Run flutter test with a live TUI.
    ///
    /// Supports the full surface of `flutter test`:
    ///   • Unit, widget, golden, integration & e2e tests.
    ///   • One or more file/dir paths positionally:
    ///       `fl test test/widget_test.dart`
    ///       `fl test integration_test/`
    ///       `fl test test/login test/signup`
    ///   • Named filters: `--name` (regex), `--plain-name` (literal).
    ///   • Tag selection: `--tags`, `--exclude-tags` (repeatable).
    ///   • Goldens: `--update-goldens` to refresh failing golden files.
    ///   • Coverage: `--coverage` writes `coverage/lcov.info`.
    ///   • Tunings: `--reporter`, `--concurrency`.
    ///   • Anything else: pass after `--`, e.g.
    ///       `fl test -- --start-paused --total-shards 4`.
    Test {
        #[arg(short, long)] project: Option<PathBuf>,
        /// Target device for integration / e2e tests (id, name prefix,
        /// or `all`). Equivalent to `flutter test -d <id>`.
        #[arg(short = 'd', long)] device: Option<String>,
        /// Run tests whose name matches this regular expression.
        #[arg(short, long)] name: Option<String>,
        /// Run tests whose name contains this literal substring.
        #[arg(long)] plain_name: Option<String>,
        /// Run only tests tagged with this value (repeatable).
        #[arg(long)] tags: Vec<String>,
        /// Skip tests tagged with this value (repeatable).
        #[arg(long)] exclude_tags: Vec<String>,
        /// Collect coverage and write to `coverage/lcov.info`.
        #[arg(long)] coverage: bool,
        /// Regenerate golden test files instead of comparing against them.
        #[arg(long)] update_goldens: bool,
        /// Shorthand: run only the golden test suite living under
        /// `test/golden/`. Equivalent to `fl test test/golden/` but
        /// pairs naturally with `--update-goldens`. Ignored when one
        /// or more explicit `paths` are given.
        #[arg(long)] golden: bool,
        /// Test reporter format: compact, expanded, github, json.
        #[arg(long)] reporter: Option<String>,
        /// Maximum number of test suites to run in parallel.
        #[arg(short = 'j', long)] concurrency: Option<u32>,
        /// Test files or directories to run. Defaults to `test/` when
        /// omitted, matching `flutter test`'s default behaviour.
        paths: Vec<String>,
        /// Pass-through args forwarded verbatim to `flutter test`.
        /// Use a `--` separator: `fl test -- --start-paused --shard-index 0`.
        #[arg(last = true, allow_hyphen_values = true)] extra: Vec<String>,
    },
    /// Emit a shell shim that hijacks `flutter` so `flutter run`,
    /// `flutter test`, `flutter build`, `flutter devices` flow through
    /// `fl` automatically (with the TUI), while every other
    /// `flutter <cmd>` keeps calling the real `flutter` binary.
    ///
    /// Usage (one-shot, in `~/.zshrc` / `~/.bashrc` / `~/.config/fish/config.fish`):
    ///
    ///     eval "$(fl init zsh)"     # or bash / fish
    ///
    /// After that, `flutter run` opens the `fl` dashboard directly —
    /// IDEs that call the `flutter` binary still bypass the shim and
    /// get vanilla behaviour, which is what you want.
    Init {
        #[arg(value_enum)] shell: ShellKind,
    },
    /// Forward any other subcommand verbatim to `flutter` with stdio
    /// inherited. Lets `fl doctor`, `fl clean`, `fl analyze`, etc. work
    /// out of the box without us re-implementing each Flutter command
    /// as a custom TUI. The first element of the Vec is the subcommand
    /// itself, the rest are its args.
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ShellKind {
    Zsh,
    Bash,
    Fish,
}

/// Resolve `--release` / `--profile` / `--debug` boolean flags into a
/// `BuildMode`, falling back to `default` when none is set. Clap's
/// `conflicts_with*` rules already guarantee at most one is true, so
/// the priority order here is just defensive.
pub fn build_mode_from_flags(release: bool, profile: bool, debug: bool, default: BuildMode) -> BuildMode {
    if release { BuildMode::Release }
    else if profile { BuildMode::Profile }
    else if debug { BuildMode::Debug }
    else { default }
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
            Cmd::Run { device, no_wifi, release, profile, debug, all, .. } => {
                assert_eq!(device, vec!["1.2.3.4:5555".to_string()]);
                assert!(no_wifi);
                assert!(!all);
                assert!(!release && !profile && !debug);
                assert_eq!(
                    build_mode_from_flags(release, profile, debug, BuildMode::Debug),
                    BuildMode::Debug,
                );
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
    fn parses_run_with_release_flag() {
        let c = Cli::parse_from(["fl", "run", "--release"]);
        match c.cmd {
            Cmd::Run { release, profile, debug, .. } => {
                assert!(release);
                assert!(!profile && !debug);
                assert_eq!(
                    build_mode_from_flags(release, profile, debug, BuildMode::Debug),
                    BuildMode::Release,
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_run_with_passthrough_extra_args() {
        let c = Cli::parse_from([
            "fl", "run", "--release", "--",
            "--flavor", "prod", "--dart-define=API=https://x",
        ]);
        match c.cmd {
            Cmd::Run { release, extra, .. } => {
                assert!(release);
                assert_eq!(
                    extra,
                    vec![
                        "--flavor".to_string(),
                        "prod".to_string(),
                        "--dart-define=API=https://x".to_string(),
                    ],
                );
            }
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
            Cmd::Build { target, release, profile, debug, .. } => {
                assert_eq!(target.as_deref(), Some("apk"));
                // Default mode for `build` is release (unlike `run`).
                assert_eq!(
                    build_mode_from_flags(release, profile, debug, BuildMode::Release),
                    BuildMode::Release,
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_build_without_target_to_passthrough() {
        // `fl build` alone should leave target=None so main can fall
        // through to plain `flutter build` (which prints the subcommand
        // list). No clap error, no required-arg failure.
        let c = Cli::parse_from(["fl", "build"]);
        match c.cmd {
            Cmd::Build { target, .. } => assert_eq!(target, None),
            _ => panic!(),
        }
    }

    #[test]
    fn unknown_subcommand_falls_through_to_external_pass_through() {
        // doctor / clean / analyze / anything-we-haven't-claimed should
        // be forwarded verbatim to the `flutter` binary.
        let c = Cli::parse_from(["fl", "doctor"]);
        assert!(matches!(&c.cmd, Cmd::External(args) if args == &vec!["doctor".to_string()]));

        let c = Cli::parse_from(["fl", "clean", "--no-color"]);
        match c.cmd {
            Cmd::External(args) => {
                assert_eq!(args, vec!["clean".to_string(), "--no-color".to_string()]);
            }
            _ => panic!("expected External fall-through"),
        }

        let c = Cli::parse_from(["fl", "analyze", "lib/main.dart"]);
        match c.cmd {
            Cmd::External(args) => {
                assert_eq!(
                    args,
                    vec!["analyze".to_string(), "lib/main.dart".to_string()]
                );
            }
            _ => panic!(),
        }
    }
}
