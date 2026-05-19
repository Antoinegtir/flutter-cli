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
        .join("target/debug/flutter-cli")
        .canonicalize()
        .expect("flutter-cli binary built — run `cargo build --bin flutter-cli` first");
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
        .args(["build", "--bin", "flutter-cli"])
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

#[test]
fn headless_wifi_drop_emits_reconnecting_and_reconnected() {
    ensure_binary_built();

    // Clean any leftover state from prior runs of the faux adb.
    let _ = std::fs::remove_dir_all("/tmp/fl-fake-adb");

    let exe = workspace_root().join("target/debug/flutter-cli").canonicalize().expect("flutter-cli binary built");
    let fixture_bin = fixtures().join("bin").canonicalize().expect("fixtures bin dir");
    let scenario_path = fixtures().join("scenarios/wifi_drop.txt").canonicalize().expect("scenario file");

    let path = format!(
        "{}:{}",
        fixture_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // We need real time so the manager's backoff actually elapses; the scenario is short.
    let out = Command::new(&exe)
        .args(["run", "--device", "1.2.3.4:5555"])
        .env("PATH", path)
        .env("FL_HEADLESS", "1")
        .env("FL_FLUTTER_SCENARIO", &scenario_path)
        .env("FL_ADB_CONNECT_FAILS_FIRST_N", "2")
        .env_remove("FLUTTER_ROOT")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn fl");

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(stdout.contains("AppStarted"), "missing AppStarted:\n{stdout}");
    assert!(stdout.contains("Stopped"), "missing Stopped:\n{stdout}");
}

fn run_fl_with_env(args: &[&str], envs: &[(&str, &std::path::Path)]) -> String {
    let exe = workspace_root().join("target/debug/flutter-cli").canonicalize().expect("fl built");
    let fixture_bin = fixtures().join("bin").canonicalize().expect("fixtures bin");
    let path = format!(
        "{}:{}",
        fixture_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut cmd = Command::new(&exe);
    cmd.args(args).env("PATH", path).env("FL_HEADLESS", "1").env_remove("FLUTTER_ROOT");
    for (k, p) in envs {
        cmd.env(k, p.canonicalize().expect("env path"));
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = cmd.output().expect("spawn fl");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn pubspec_in_workspace() -> PathBuf {
    // tests need a real pubspec.yaml for pre-checks in build/test/pub/clean.
    let p = workspace_root().join("target/test-pubspec");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("pubspec.yaml"), "name: dummy\n").unwrap();
    std::fs::create_dir_all(p.join("test")).unwrap();
    p
}

#[test]
fn headless_build_emits_progress_and_built() {
    ensure_binary_built();
    let pubspec = pubspec_in_workspace();
    let scenario = fixtures().join("scenarios/build_apk.txt");
    let out = run_fl_with_env(
        &["build", "apk", "--project", pubspec.to_str().unwrap()],
        &[("FL_FLUTTER_BUILD_SCENARIO", &scenario)],
    );
    assert!(out.contains("Progress"), "missing progress events:\n{out}");
    assert!(out.contains("Built"), "missing Built line:\n{out}");
}

#[test]
fn headless_test_emits_test_events() {
    ensure_binary_built();
    let pubspec = pubspec_in_workspace();
    let scenario = fixtures().join("scenarios/test_basic.txt");
    let out = run_fl_with_env(
        &["test", "--project", pubspec.to_str().unwrap()],
        &[("FL_FLUTTER_TEST_SCENARIO", &scenario)],
    );
    assert!(out.contains("TestStarted"), "missing TestStarted:\n{out}");
    assert!(out.contains("AllDone"), "missing AllDone:\n{out}");
}

// `fl doctor` and `fl clean` are no longer handled by `fl` — they now
// pass through to the real `flutter` binary. The previous integration
// tests asserted internal Section/Done events from our own TUIs; with
// the pass-through model there's nothing fl-specific left to test.

#[test]
fn headless_multi_device_emits_two_app_started() {
    ensure_binary_built();
    let _ = std::fs::remove_dir_all("/tmp/fl-fake-adb");

    let pubspec = pubspec_in_workspace();
    let devices_file = fixtures().join("scenarios/multi_devices.txt");
    let scenario = fixtures().join("scenarios/nominal.txt");
    let out = run_fl_with_env(
        &[
            "run",
            "--no-picker", "--no-wifi",
            "--device", "DEV1",
            "--device", "DEV2",
            "--project", pubspec.to_str().unwrap(),
        ],
        &[
            ("FL_ADB_FIXTURE_DEVICES", &devices_file),
            ("FL_FLUTTER_SCENARIO", &scenario),
        ],
    );
    let starts = out.matches("AppStarted").count();
    assert!(starts >= 2, "expected ≥ 2 AppStarted events, output:\n{out}");
}

#[test]
fn headless_ios_run_emits_app_started() {
    ensure_binary_built();
    let _ = std::fs::remove_dir_all("/tmp/fl-fake-adb");

    let pubspec = pubspec_in_workspace();
    let devicectl_scenario = fixtures().join("scenarios/ios_one_device.json");
    let flutter_scenario = fixtures().join("scenarios/nominal.txt");

    // Empty adb devices list — only iOS device present.
    let empty_adb = workspace_root().join("target/test-empty-adb-devices.txt");
    std::fs::write(&empty_adb, "List of devices attached\n").unwrap();

    let out = run_fl_with_env(
        &[
            "run",
            "--no-picker", "--no-wifi",
            "--device", "00008140-0011002233",
            "--project", pubspec.to_str().unwrap(),
        ],
        &[
            ("FL_ADB_FIXTURE_DEVICES", &empty_adb),
            ("FL_XCRUN_DEVICECTL_SCENARIO", &devicectl_scenario),
            ("FL_FLUTTER_SCENARIO", &flutter_scenario),
        ],
    );
    assert!(out.contains("AppStarted"), "missing AppStarted, output:\n{out}");
    assert!(!out.contains("pre-pair failed"), "iOS device wrongly triggered pre-pair:\n{out}");
}
