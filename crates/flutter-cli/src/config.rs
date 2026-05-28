//! Project-level configuration for flutter-cli, read from `pubspec.yaml`.
//!
//! Conventions
//! ───────────
//! Every Flutter project already ships a `pubspec.yaml` at its root. We
//! piggy-back on it for our own config so the user has nothing extra to
//! check in. A new top-level `flutter_cli:` section, ignored by Flutter
//! itself, holds our keys:
//!
//! ```yaml
//! flutter_cli:
//!   pre_run:
//!     - dart run build_runner build --delete-conflicting-outputs
//!     - dart format lib/
//!   pre_test:
//!     - dart run build_runner build
//!   pre_build:
//!     - ./tool/check_env.sh
//! ```
//!
//! Each `pre_<subcmd>` list is run **before** the TUI initializes for
//! that subcommand. Commands are spawned through `sh -c` so the user
//! can use the shell pipes / globbing they already know. The hook
//! aborts on the first non-zero exit — broken codegen should NOT
//! silently let the dashboard come up with stale generated files.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::Stdio;

/// Top-level pubspec shape — we only care about our own section, the
/// rest is left to `flutter` and `pub`. `#[serde(default)]` on every
/// field means a project without a `flutter_cli:` block deserializes
/// fine into a `FlutterCliConfig::default()` (all hook lists empty).
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Pubspec {
    #[serde(default, rename = "flutter_cli")]
    pub flutter_cli: FlutterCliConfig,
}

/// The `flutter_cli:` section of pubspec.yaml. Every key is optional.
/// Renamed serde fields keep the YAML snake_case while letting us use
/// idiomatic Rust naming.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
pub struct FlutterCliConfig {
    #[serde(default)]
    pub pre_run: Vec<String>,
    #[serde(default)]
    pub pre_test: Vec<String>,
    #[serde(default)]
    pub pre_build: Vec<String>,
}

impl FlutterCliConfig {
    /// Return the pre-hook list for a given subcommand name. `subcmd`
    /// must be one of `"run" | "test" | "build"`. Anything else
    /// returns an empty slice — callers don't have to special-case.
    pub fn pre_hooks_for(&self, subcmd: &str) -> &[String] {
        match subcmd {
            "run" => &self.pre_run,
            "test" => &self.pre_test,
            "build" => &self.pre_build,
            _ => &[],
        }
    }
}

/// Read `<project>/pubspec.yaml` and parse out the `flutter_cli:` block.
/// A missing file or missing section both yield a default-empty
/// `FlutterCliConfig` so callers can blindly call `.pre_hooks_for()`
/// without worrying about which case they hit.
pub fn load_pubspec_config(project_root: &Path) -> Result<FlutterCliConfig> {
    let path = project_root.join("pubspec.yaml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No pubspec at all — likely a wrong directory, but that's
            // not config's job to enforce. Hand back an empty config
            // and let the subcommand's existing pubspec check fire.
            return Ok(FlutterCliConfig::default());
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let pubspec: Pubspec = serde_yml::from_str(&raw)
        .with_context(|| format!("parsing {} (flutter_cli section)", path.display()))?;
    Ok(pubspec.flutter_cli)
}

/// Run every pre-hook for `subcmd` in declared order, streaming each
/// command's stdout/stderr to the user's terminal AS IT HAPPENS (no
/// buffering — codegen output should scroll live so the user sees
/// progress). Aborts on the first non-zero exit.
///
/// The hook is intentionally run BEFORE the TUI initializes — that
/// way the alternate screen / inline viewport hasn't taken over the
/// terminal yet and the hook's output stays visible in scrollback
/// after the TUI exits.
///
/// Each command is run from `project_root` with the same env the
/// parent inherited. We use `sh -c "..."` so the user can write
/// pipes (`a | b`), redirects, `&&`, env-var substitution — anything
/// they'd type at a shell prompt.
pub async fn run_pre_hooks(subcmd: &str, project_root: &Path) -> Result<()> {
    let config = load_pubspec_config(project_root)?;
    let hooks = config.pre_hooks_for(subcmd);
    if hooks.is_empty() {
        return Ok(());
    }

    // Banner-ish leading line so the user can tell where the codegen
    // output starts when their hook is noisy. ANSI bold + dim so it
    // stands out without an emoji that might not render in every term.
    eprintln!(
        "\x1b[1mflutter-cli\x1b[0m \x1b[2mpre_{subcmd}\x1b[0m · {n} hook{s}",
        n = hooks.len(),
        s = if hooks.len() == 1 { "" } else { "s" }
    );

    for (i, cmd) in hooks.iter().enumerate() {
        eprintln!("\x1b[2m  [{}/{}]\x1b[0m \x1b[36m$ {cmd}\x1b[0m", i + 1, hooks.len());
        let status = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(project_root)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .with_context(|| format!("spawning pre_{subcmd} hook: {cmd}"))?;
        if !status.success() {
            anyhow::bail!(
                "pre_{subcmd} hook failed (exit {code}): {cmd}",
                code = status.code().unwrap_or(-1)
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Avoid pulling in the `tempfile` crate just for these tests —
    /// write to a deterministic, per-test subdir under the OS temp dir.
    fn write_pubspec(slug: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("flutter_cli_config_tests")
            .join(slug);
        let _ = std::fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("pubspec.yaml"), body).unwrap();
        dir
    }

    fn empty_dir(slug: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("flutter_cli_config_tests")
            .join(slug);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_pubspec_returns_default_empty_config() {
        let dir = empty_dir("no_pubspec");
        let cfg = load_pubspec_config(&dir).unwrap();
        assert!(cfg.pre_run.is_empty());
        assert!(cfg.pre_test.is_empty());
        assert!(cfg.pre_build.is_empty());
    }

    #[test]
    fn pubspec_without_section_returns_default() {
        let dir = write_pubspec(
            "no_section",
            "name: my_app\nversion: 0.1.0\nflutter:\n  uses-material-design: true\n",
        );
        let cfg = load_pubspec_config(&dir).unwrap();
        assert!(cfg.pre_run.is_empty());
    }

    #[test]
    fn pubspec_with_pre_run_parses_commands_in_order() {
        let dir = write_pubspec(
            "pre_run",
            "name: app\nflutter_cli:\n  pre_run:\n    - cmd1\n    - cmd2\n    - echo hi\n",
        );
        let cfg = load_pubspec_config(&dir).unwrap();
        assert_eq!(cfg.pre_run, vec!["cmd1", "cmd2", "echo hi"]);
        assert!(cfg.pre_test.is_empty());
        assert!(cfg.pre_build.is_empty());
    }

    #[test]
    fn pre_hooks_for_returns_right_list_per_subcommand() {
        let dir = write_pubspec(
            "per_subcmd",
            "name: app\nflutter_cli:\n  pre_run: [run-1]\n  pre_test: [test-1]\n  pre_build: [build-1, build-2]\n",
        );
        let cfg = load_pubspec_config(&dir).unwrap();
        assert_eq!(cfg.pre_hooks_for("run"), &["run-1".to_string()]);
        assert_eq!(cfg.pre_hooks_for("test"), &["test-1".to_string()]);
        assert_eq!(
            cfg.pre_hooks_for("build"),
            &["build-1".to_string(), "build-2".to_string()]
        );
        assert!(cfg.pre_hooks_for("doctor").is_empty());
    }

    #[tokio::test]
    async fn run_pre_hooks_runs_commands_and_succeeds() {
        let dir = write_pubspec(
            "happy_path",
            "name: app\nflutter_cli:\n  pre_run:\n    - true\n    - 'echo ok'\n",
        );
        run_pre_hooks("run", &dir).await.unwrap();
    }

    #[tokio::test]
    async fn run_pre_hooks_aborts_on_first_failure() {
        let dir = write_pubspec(
            "failing",
            "name: app\nflutter_cli:\n  pre_run:\n    - false\n    - 'echo should-not-run'\n",
        );
        let res = run_pre_hooks("run", &dir).await;
        assert!(res.is_err(), "expected error from failing hook");
        let msg = format!("{}", res.unwrap_err());
        assert!(msg.contains("false"), "error should name the failing cmd: {msg}");
    }

    #[tokio::test]
    async fn run_pre_hooks_is_noop_without_config() {
        let dir = write_pubspec("no_hooks", "name: app\nversion: 0.0.1\n");
        run_pre_hooks("run", &dir).await.unwrap();
    }
}
