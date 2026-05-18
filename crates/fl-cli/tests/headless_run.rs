//! Drive the `fl` binary in headless mode against a faux flutter scenario.

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points to crates/fl-cli; up two levels = workspace root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> PathBuf {
    workspace_root().join("tests/fixtures")
}

fn run_fl_with(scenario: &str) -> String {
    let exe = workspace_root()
        .join("target/debug/fl")
        .canonicalize()
        .expect("fl binary built — run `cargo build --bin fl` first");
    let fixture_bin = fixtures()
        .join("bin")
        .canonicalize()
        .expect("fixtures bin dir");
    let scenario_path = fixtures()
        .join("scenarios")
        .join(scenario)
        .canonicalize()
        .expect("scenario file");

    let path = format!(
        "{}:{}",
        fixture_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(&exe)
        .args(["run", "--no-wifi", "--device", "ABC123"])
        .env("PATH", path)
        .env("FL_HEADLESS", "1")
        .env("FL_FLUTTER_SCENARIO", &scenario_path)
        .env_remove("FLUTTER_ROOT")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn fl");

    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn ensure_binary_built() {
    let status = Command::new(env!("CARGO"))
        .args(["build", "--bin", "fl"])
        .status()
        .expect("cargo build");
    assert!(status.success());
}

#[test]
fn headless_nominal_emits_app_started_and_stop() {
    ensure_binary_built();
    let out = run_fl_with("nominal.txt");
    assert!(
        out.contains("AppStarted"),
        "missing AppStarted in output:\n{out}"
    );
    assert!(
        out.contains("Stopped"),
        "missing Stopped in output:\n{out}"
    );
}

#[test]
fn headless_no_device_scenario_reports_error_log() {
    ensure_binary_built();
    let out = run_fl_with("no_device.txt");
    assert!(
        out.to_lowercase().contains("no supported devices"),
        "missing error message in output:\n{out}"
    );
}
