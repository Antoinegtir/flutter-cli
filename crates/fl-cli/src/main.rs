mod build_cmd;
mod cli;
mod devices_cmd;
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = init_logging().ok();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Devices => devices_cmd::run().await,
        Cmd::Run { project, device, no_wifi, mode } => run_cmd::run(project, device, no_wifi, mode).await,
    }
}
