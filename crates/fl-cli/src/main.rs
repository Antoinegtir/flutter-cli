mod build_cmd;
mod cli;
mod devices_cmd;
mod external_cmd;
mod multi;
mod pub_cmd;
mod run_cmd;
mod test_cmd;

use anyhow::Context;
use clap::Parser;
use cli::{Cli, Cmd};
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
        Cmd::Run { project, device, all, no_picker, no_wifi, no_tui, mode } => {
            run_cmd::run(project, device, all, no_picker, no_wifi, no_tui, mode).await
        }
        Cmd::Build { target, project, mode } => build_cmd::run(target, project, mode).await,
        Cmd::Test { project, name } => test_cmd::run(project, name).await,
        Cmd::Pub { sub, project } => pub_cmd::run(sub, project).await,
        Cmd::External(args) => external_cmd::run(args).await,
    }
}
