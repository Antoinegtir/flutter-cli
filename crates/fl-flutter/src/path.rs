//! Resolve the `flutter` binary. Checks (in order):
//! 1. Explicit override (from Config)
//! 2. Project-pinned FVM version (`.fvm/flutter_sdk` symlink, else `.fvmrc` /
//!    `.fvm/fvm_config.json` resolved against `~/fvm/versions/<version>`)
//! 3. `FLUTTER_ROOT` env
//! 4. `flutter` in PATH
//! 5. Conventional install locations under `$HOME`

use std::path::{Path, PathBuf};

const CONVENTIONAL: &[&str] = &[
    "fvm/default/bin/flutter",
    "development/flutter/bin/flutter",
    "flutter/bin/flutter",
];

pub fn resolve_flutter(
    explicit: Option<&Path>,
    project_dir: Option<&Path>,
    env_root: Option<&str>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    if let Some(project) = project_dir {
        if let Some(p) = resolve_fvm_flutter(project, home) {
            return Some(p);
        }
    }
    if let Some(root) = env_root {
        let p = Path::new(root).join("bin/flutter");
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(found) = which_in_path("flutter") {
        return Some(found);
    }
    if let Some(home) = home {
        for rel in CONVENTIONAL {
            let p = home.join(rel);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// Resolve the `flutter` binary pinned by FVM for [project], if any.
///
/// Prefers the `<project>/.fvm/flutter_sdk` symlink that `fvm use` creates
/// (also what IDEs read). Falls back to the version recorded in `.fvmrc`
/// (or the legacy `.fvm/fvm_config.json`) resolved against the FVM cache
/// (`~/fvm/versions/<version>/bin/flutter`) — useful right after a fresh
/// clone where the symlink is gitignored but `.fvmrc` is committed.
fn resolve_fvm_flutter(project: &Path, home: Option<&Path>) -> Option<PathBuf> {
    // 1. The symlink `fvm use` creates (also what IDEs point at). This works
    //    regardless of where the FVM cache lives, since it's a direct link.
    let symlinked = project.join(".fvm/flutter_sdk/bin/flutter");
    if symlinked.exists() {
        return Some(symlinked);
    }
    // 2. Fall back to the version pinned in `.fvmrc` / legacy config, resolved
    //    against the FVM cache (`~/fvm/versions/<version>`). Covers fresh
    //    clones where the symlink is gitignored but `.fvmrc` is committed.
    let version = read_fvm_pinned_version(project)?;
    let cached = home?
        .join("fvm/versions")
        .join(&version)
        .join("bin/flutter");
    cached.exists().then_some(cached)
}

/// Read the Flutter version pinned by FVM for [project], from `.fvmrc`
/// (key `flutter`) or the legacy `.fvm/fvm_config.json` (key
/// `flutterSdkVersion`). Returns the raw version/channel string (e.g.
/// `"3.27.1"` or `"stable"`), which doubles as the FVM cache folder name.
fn read_fvm_pinned_version(project: &Path) -> Option<String> {
    fn read_key(path: PathBuf, key: &str) -> Option<String> {
        let contents = std::fs::read_to_string(path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&contents).ok()?;
        let s = value.get(key)?.as_str()?;
        (!s.is_empty()).then(|| s.to_string())
    }
    read_key(project.join(".fvmrc"), "flutter")
        .or_else(|| read_key(project.join(".fvm/fvm_config.json"), "flutterSdkVersion"))
}

/// The Flutter framework version and bundled Dart SDK version of a
/// resolved SDK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkVersions {
    pub flutter: String,
    pub dart: String,
}

/// Read the Flutter and Dart versions for the SDK that owns [flutter_bin]
/// (expected at `<sdk>/bin/flutter`), from `<sdk>/bin/cache/flutter.version.json`.
///
/// Returns `None` if the file is missing or malformed (e.g. an SDK that
/// hasn't been set up yet) — callers should degrade gracefully.
pub fn sdk_versions(flutter_bin: &Path) -> Option<SdkVersions> {
    // <sdk>/bin/flutter -> bin -> <sdk>
    let sdk_root = flutter_bin.parent()?.parent()?;
    let json = sdk_root.join("bin/cache/flutter.version.json");
    let contents = std::fs::read_to_string(json).ok()?;
    let value: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let flutter = value.get("frameworkVersion")?.as_str()?.to_string();
    let dart = value.get("dartSdkVersion")?.as_str()?.to_string();
    if flutter.is_empty() || dart.is_empty() {
        return None;
    }
    Some(SdkVersions { flutter, dart })
}

fn which_in_path(name: &str) -> std::io::Result<PathBuf> {
    let path = std::env::var_os("PATH").ok_or_else(|| std::io::Error::other("no PATH"))?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(std::io::Error::other("not found in PATH"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn explicit_override_wins_when_present() {
        let dir = tempdir();
        let exe = dir.join("flutter");
        fs::write(&exe, "").unwrap();
        let p = resolve_flutter(Some(&exe), None, None, None).unwrap();
        assert_eq!(p, exe);
    }

    #[test]
    fn env_root_resolves_to_bin_flutter() {
        let dir = tempdir();
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("flutter");
        fs::write(&exe, "").unwrap();
        let p = resolve_flutter(None, None, Some(dir.to_str().unwrap()), None).unwrap();
        assert_eq!(p, exe);
    }

    #[test]
    fn conventional_path_under_home_resolves() {
        let dir = tempdir();
        let exe = dir.join("development/flutter/bin/flutter");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, "").unwrap();
        // Empty PATH so which_in_path returns Err and falls through to conventional check.
        let _save = std::env::var_os("PATH");
        std::env::set_var("PATH", "");
        let p = resolve_flutter(None, None, None, Some(&dir));
        if let Some(save) = _save {
            std::env::set_var("PATH", save);
        }
        assert_eq!(p, Some(exe));
    }

    #[test]
    fn returns_none_when_no_candidate_exists() {
        let dir = tempdir();
        // Subdir that does not match any conventional path.
        let home = dir.join("empty_home");
        fs::create_dir_all(&home).unwrap();
        // Empty PATH so which_in_path fails.
        let _save = std::env::var_os("PATH");
        std::env::set_var("PATH", "");
        let p = resolve_flutter(None, None, None, Some(&home));
        if let Some(save) = _save {
            std::env::set_var("PATH", save);
        }
        assert!(p.is_none());
    }

    #[test]
    fn fvm_project_sdk_symlink_resolves() {
        let dir = tempdir();
        let project = dir.join("myapp");
        let sdk_flutter = project.join(".fvm/flutter_sdk/bin/flutter");
        fs::create_dir_all(sdk_flutter.parent().unwrap()).unwrap();
        fs::write(&sdk_flutter, "").unwrap();
        // Project FVM wins even when FLUTTER_ROOT is also set.
        let other = dir.join("global");
        let other_bin = other.join("bin");
        fs::create_dir_all(&other_bin).unwrap();
        fs::write(other_bin.join("flutter"), "").unwrap();
        let p = resolve_flutter(None, Some(&project), Some(other.to_str().unwrap()), None);
        assert_eq!(p, Some(sdk_flutter));
    }

    #[test]
    fn fvmrc_version_resolves_from_cache() {
        let dir = tempdir();
        let project = dir.join("myapp");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join(".fvmrc"), r#"{"flutter": "3.27.1"}"#).unwrap();
        let home = dir.join("home");
        let cache_flutter = home.join("fvm/versions/3.27.1/bin/flutter");
        fs::create_dir_all(cache_flutter.parent().unwrap()).unwrap();
        fs::write(&cache_flutter, "").unwrap();
        let p = resolve_flutter(None, Some(&project), None, Some(&home));
        assert_eq!(p, Some(cache_flutter));
    }

    #[test]
    fn legacy_fvm_config_version_resolves_from_cache() {
        let dir = tempdir();
        let project = dir.join("myapp");
        fs::create_dir_all(project.join(".fvm")).unwrap();
        fs::write(
            project.join(".fvm/fvm_config.json"),
            r#"{"flutterSdkVersion": "3.19.6", "flavors": {}}"#,
        )
        .unwrap();
        let home = dir.join("home");
        let cache_flutter = home.join("fvm/versions/3.19.6/bin/flutter");
        fs::create_dir_all(cache_flutter.parent().unwrap()).unwrap();
        fs::write(&cache_flutter, "").unwrap();
        let p = resolve_flutter(None, Some(&project), None, Some(&home));
        assert_eq!(p, Some(cache_flutter));
    }

    #[test]
    fn no_fvm_pin_falls_through_to_normal_resolution() {
        let dir = tempdir();
        let project = dir.join("myapp");
        fs::create_dir_all(&project).unwrap();
        // No .fvmrc / .fvm here, so a FLUTTER_ROOT should still win.
        let root = dir.join("sdk");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("flutter");
        fs::write(&exe, "").unwrap();
        let p = resolve_flutter(None, Some(&project), Some(root.to_str().unwrap()), None);
        assert_eq!(p, Some(exe));
    }

    #[test]
    fn sdk_versions_reads_flutter_version_json() {
        let dir = tempdir();
        let sdk = dir.join("flutter");
        let cache = sdk.join("bin/cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(
            cache.join("flutter.version.json"),
            r#"{"frameworkVersion":"3.41.9","channel":"stable","dartSdkVersion":"3.11.5"}"#,
        )
        .unwrap();
        let v = sdk_versions(&sdk.join("bin/flutter")).unwrap();
        assert_eq!(v.flutter, "3.41.9");
        assert_eq!(v.dart, "3.11.5");
    }

    #[test]
    fn sdk_versions_none_when_file_missing() {
        let dir = tempdir();
        assert!(sdk_versions(&dir.join("flutter/bin/flutter")).is_none());
    }

    fn tempdir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("fl-test-{}", uuid_like()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{nanos:x}")
    }
}
