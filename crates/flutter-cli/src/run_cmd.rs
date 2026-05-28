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
