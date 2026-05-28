//! `flutter-cli run` — delegates to multi::run_multi for the actual orchestration.

use fl_core::BuildMode;
use std::path::PathBuf;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    project: Option<PathBuf>,
    devices_arg: Vec<String>,
    all: bool,
    no_picker: bool,
    no_wifi: bool,
    no_tui: bool,
    mode: BuildMode,
    extra: Vec<String>,
) -> anyhow::Result<()> {
    // Resolve the project root the SAME way run_multi will. We need it
    // here, BEFORE the TUI takes over the terminal, so the pre-run
    // hooks (codegen, lint, env-checks) can stream their output into
    // the user's scrollback unobstructed.
    let project_root = project
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    crate::config::run_pre_hooks("run", &project_root).await?;

    crate::multi::run_multi(
        project,
        devices_arg,
        all,
        no_picker,
        no_wifi,
        no_tui,
        mode,
        extra,
    )
    .await
}
