//! Resolve the `flutter` binary. Checks (in order):
//! 1. Explicit override (from Config)
//! 2. `FLUTTER_ROOT` env
//! 3. `flutter` in PATH
//! 4. Conventional install locations under `$HOME`

use std::path::{Path, PathBuf};

const CONVENTIONAL: &[&str] = &[
    "fvm/default/bin/flutter",
    "development/flutter/bin/flutter",
    "flutter/bin/flutter",
];

pub fn resolve_flutter(explicit: Option<&Path>, env_root: Option<&str>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        if p.exists() {
            return Some(p.to_path_buf());
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
        let p = resolve_flutter(Some(&exe), None, None).unwrap();
        assert_eq!(p, exe);
    }

    #[test]
    fn env_root_resolves_to_bin_flutter() {
        let dir = tempdir();
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("flutter");
        fs::write(&exe, "").unwrap();
        let p = resolve_flutter(None, Some(dir.to_str().unwrap()), None).unwrap();
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
        let p = resolve_flutter(None, None, Some(&dir));
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
        let p = resolve_flutter(None, None, Some(&home));
        if let Some(save) = _save {
            std::env::set_var("PATH", save);
        }
        assert!(p.is_none());
    }

    fn tempdir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("fl-test-{}", uuid_like()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        format!("{nanos:x}")
    }
}
