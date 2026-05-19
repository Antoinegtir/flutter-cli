//! Pass-through to the real `flutter` binary for any subcommand that
//! `fl` doesn't claim itself. Inherits stdio so output (including
//! progress bars / TTY-detected coloured output) flows through as if
//! the user had typed `flutter` directly. Exits with the same code as
//! the child process.

use anyhow::Context;
use std::process::Stdio;

pub async fn run(args: Vec<String>) -> anyhow::Result<()> {
    let flutter = crate::multi::resolve_flutter_path()
        .context("locating flutter binary for pass-through")?;
    let status = tokio::process::Command::new(&flutter)
        .args(&args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .with_context(|| format!("spawning {} {}", flutter.display(), args.join(" ")))?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
