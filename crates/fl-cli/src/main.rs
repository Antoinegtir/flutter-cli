mod build_cmd;
mod cli;
mod devices_cmd;
mod external_cmd;
mod init_cmd;
mod multi;
mod run_cmd;
mod test_cmd;

use anyhow::Context;
use clap::Parser;
use cli::{build_mode_from_flags, Cli, Cmd};
use fl_core::BuildMode;
use directories::ProjectDirs;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

fn init_logging() -> anyhow::Result<tracing_appender::non_blocking::WorkerGuard> {
    let dirs = ProjectDirs::from("", "", "fl").context("project dirs")?;
    let log_dir = dirs.cache_dir();
    std::fs::create_dir_all(log_dir)?;
    let appender = RollingFileAppender::new(Rotation::NEVER, log_dir, "fl.log");
    let (nb, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env("FL_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(nb).with_ansi(false))
        .init();
    Ok(guard)
}

/// Ensure the terminal is always returned to a sane state when the process
/// dies — even from a panic in TUI rendering code. Without this, a crash
/// inside the TUI loop would leave raw mode on and the alt-screen active.
fn install_terminal_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let _ = crossterm::terminal::disable_raw_mode();
        // Disable ALL mouse tracking modes that might be on (matches
        // TuiRunner::restore). Skipping this leaves the user's
        // terminal forwarding raw mouse escapes ("^[[<35;…M") into
        // their shell after a crash.
        let mut out = std::io::stdout();
        let _ = write!(
            out,
            "\x1b[?1006l\x1b[?1015l\x1b[?1005l\x1b[?1004l\x1b[?1003l\x1b[?1002l\x1b[?1000l"
        );
        let _ = out.flush();
        let _ = crossterm::execute!(out, crossterm::terminal::LeaveAlternateScreen);
        default_hook(info);
    }));
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    install_terminal_panic_hook();
    let _guard = init_logging().ok();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Devices => devices_cmd::run().await,
        Cmd::Run { project, device, all, no_picker, no_wifi, no_tui, basic, release, profile, debug, extra } => {
            let mode = build_mode_from_flags(release, profile, debug, BuildMode::Debug);
            if basic {
                // Pure passthrough — exec `flutter run [-d device]
                // [mode flag] [extras…]` with the user's terminal
                // wired straight through. No TUI, no `--machine`,
                // no per-device prefix; same output as plain
                // `flutter run`.
                let mut args = vec!["run".to_string()];
                for d in &device {
                    args.push("-d".into());
                    args.push(d.clone());
                }
                if all {
                    args.push("-d".into());
                    args.push("all".into());
                }
                if !matches!(mode, BuildMode::Debug) {
                    args.push(mode.flutter_flag().to_string());
                }
                args.extend(extra);
                let _ = no_picker; let _ = no_wifi; let _ = no_tui; let _ = project;
                return external_cmd::run(args).await;
            }
            run_cmd::run(project, device, all, no_picker, no_wifi, no_tui, mode, extra).await
        }
        Cmd::Build { target, project, release, profile, debug, basic, extra } => {
            // Build defaults to release (most common use case for `flutter build`).
            let mode = build_mode_from_flags(release, profile, debug, BuildMode::Release);
            match target {
                // `--basic` skips the TUI build view and just exec's
                // `flutter build <target> [extra...]` with inherited
                // stdio. Same effect as the external pass-through.
                Some(t) if basic => {
                    let mut args = vec!["build".to_string(), t];
                    if !matches!(mode, BuildMode::Release) {
                        args.push(mode.flutter_flag().to_string());
                    }
                    args.extend(extra);
                    external_cmd::run(args).await
                }
                Some(t) => build_cmd::run(t, project, mode, extra).await,
                None => {
                    // No target → mirror `flutter build` (which prints
                    // the available subcommands). Forward verbatim
                    // through the external pass-through.
                    let mut args = vec!["build".to_string()];
                    args.extend(extra);
                    external_cmd::run(args).await
                }
            }
        }
        Cmd::Init { shell } => init_cmd::run(shell).await,
        Cmd::Test {
            project, device, name, plain_name, tags, exclude_tags,
            coverage, update_goldens, golden, reporter, concurrency, basic, paths, extra,
        } => {
            if basic {
                // `--basic` skips the TUI test runner entirely and
                // just exec's `flutter test [filters/paths…]` with
                // inherited stdio. Mirrors what users get without
                // `fl` installed.
                let mut args = vec!["test".to_string()];
                if let Some(d) = device { args.push("-d".into()); args.push(d); }
                if let Some(n) = name { args.push("--name".into()); args.push(n); }
                if let Some(n) = plain_name { args.push("--plain-name".into()); args.push(n); }
                for t in tags { args.push("--tags".into()); args.push(t); }
                for t in exclude_tags { args.push("--exclude-tags".into()); args.push(t); }
                if coverage { args.push("--coverage".into()); }
                if update_goldens { args.push("--update-goldens".into()); }
                if let Some(r) = reporter { args.push("--reporter".into()); args.push(r); }
                if let Some(c) = concurrency { args.push(format!("--concurrency={c}")); }
                args.extend(extra);
                // `--golden` defaults paths to test/golden/ when no
                // explicit paths were given — mirror the TUI flow.
                let mut paths = paths;
                if golden && paths.is_empty() {
                    paths.push("test/golden/".into());
                }
                args.extend(paths);
                return external_cmd::run(args).await;
            }
            test_cmd::run(test_cmd::Options {
                project, device, name, plain_name, tags, exclude_tags,
                coverage, update_goldens, golden, reporter, concurrency, paths, extra,
            }).await
        }
        Cmd::External(args) => external_cmd::run(args).await,
    }
}
