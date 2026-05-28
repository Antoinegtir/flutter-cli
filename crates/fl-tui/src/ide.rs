//! IDE integration: detect VS Code / Android Studio and open a Dart
//! source file at a specific line with the `e` keybind.
//!
//! Detection strategy
//! ──────────────────
//! VS Code:        $VSCODE_PID is set in VS Code's integrated terminal.
//!                 $TERM_PROGRAM=vscode covers the same case on Linux.
//!                 Falls back to probing the `code` (or `cursor` / `codium`)
//!                 binary on PATH so it works from an external terminal too.
//!
//! Android Studio: JetBrains IDEs expose a REST server on port 63342
//!                 (Settings → Tools → HTTP Requests → "Allow unsigned
//!                 requests"). We do a 100 ms TCP probe so detection is
//!                 instant even when the IDE isn't the terminal parent.
//!                 $STUDIO_VM_OPTIONS / $STUDIO_PROPERTIES also hint at it.
//!
//! IntelliJ IDEA uses the same REST API on the same port — it is handled
//! identically to Android Studio.

use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

/// Which IDE to open files in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdeKind {
    VsCode,
    AndroidStudio, // also covers IntelliJ IDEA
}

impl IdeKind {
    pub fn label(self) -> &'static str {
        match self {
            IdeKind::VsCode => "VS Code",
            IdeKind::AndroidStudio => "Android Studio",
        }
    }
}

/// Detect the IDE from environment variables and running services.
/// Cheap enough to call on every `e` keypress; cache the result yourself
/// if you want to avoid the TCP probe on repeat calls.
///
/// Priority order — `"running INSIDE this IDE right now"` always wins
/// over `"this IDE is installed somewhere"`. Without this ordering the
/// jump-to-error keybind ends up opening the WRONG editor — typically
/// VS Code while the user is sitting in Android Studio's terminal,
/// because `code` happens to be on `$PATH`.
pub fn detect() -> Option<IdeKind> {
    // ── 1. Are we INSIDE a known terminal emulator? ──────────────────────
    // Strongest signal we have: the host IDE injects an env var into the
    // shell of its own integrated terminal. Beats any "is X installed"
    // check by a mile.
    if let Ok(v) = std::env::var("TERMINAL_EMULATOR") {
        // JetBrains / Android Studio / IntelliJ — JediTerm-based.
        if v.contains("JetBrains") || v.contains("JediTerm") {
            return Some(IdeKind::AndroidStudio);
        }
    }
    if std::env::var("VSCODE_PID").is_ok()
        || std::env::var("TERM_PROGRAM").ok().as_deref() == Some("vscode")
    {
        return Some(IdeKind::VsCode);
    }

    // ── 2. Which IDE is currently RUNNING on this machine? ──────────────
    // Even if the user isn't inside an integrated terminal, a running
    // IDE beats an installed-but-not-running one. AS exposes a REST
    // server on port 63342 while it's up — quick TCP probe confirms it.
    if std::env::var("STUDIO_VM_OPTIONS").is_ok()
        || std::env::var("STUDIO_PROPERTIES").is_ok()
        || as_rest_available()
    {
        return Some(IdeKind::AndroidStudio);
    }

    // ── 3. Last resort: an IDE binary on PATH (might not be running). ──
    // Try VS Code first since the `code` CLI is the most commonly
    // installed editor on dev machines.
    for bin in &["code", "cursor", "codium", "windsurf"] {
        if which_exists(bin) {
            return Some(IdeKind::VsCode);
        }
    }

    None
}

/// Non-blocking check: can we reach port 63342 on localhost?
fn as_rest_available() -> bool {
    TcpStream::connect_timeout(
        &"127.0.0.1:63342".parse().unwrap(),
        Duration::from_millis(100),
    )
    .is_ok()
}

/// True if `name` resolves to an executable in the current PATH.
fn which_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .any(|dir| dir.join(name).exists())
        })
        .unwrap_or(false)
}

// ── File reference parsing ────────────────────────────────────────────────

/// A resolved reference to a source file found inside a Dart log message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    /// Path relative to the project root, e.g. `lib/src/auth/login.dart`.
    pub rel_path: String,
    pub line: u32,
    pub col: Option<u32>,
}

/// Dart stack frame prefixes that are considered "user-space" (i.e., belong
/// to the developer's own project). Framework paths such as
/// `package:flutter/...` are skipped so the jump always lands in the
/// developer's own code rather than the Flutter source.
///
/// A `package:<name>/` path is only treated as user-space when `<name>` is
/// NOT one of the well-known SDK/framework packages listed below.
const FRAMEWORK_PACKAGES: &[&str] = &[
    "flutter",
    "flutter_test",
    "flutter_web_plugins",
    "dart",
    "sky_engine",
    "_flutter_web_sdk",
    "collection",
    "material_color_utilities",
    "meta",
    "vector_math",
];

/// Extract the first user-space `.dart:LINE[:COL]` reference from `message`.
/// Returns `None` when the line has no dart path, or only framework paths.
pub fn extract_file_ref(message: &str) -> Option<FileRef> {
    if !message.contains(".dart:") {
        return None;
    }

    // Walk the string looking for ".dart:" followed by digits.
    let mut search = message;
    while let Some(pos) = search.find(".dart:") {
        // Slice from the start of the remaining string up through ".dart"
        let before_colon = &search[..pos + 5]; // includes ".dart"

        // Walk backwards from pos to find the start of the path token.
        let token_start = before_colon
            .rfind(|c: char| c.is_whitespace() || c == '(' || c == '\'' || c == '"' || c == '>')
            .map(|i| i + 1)
            .unwrap_or(0);
        let raw_path = &before_colon[token_start..]; // e.g. "package:myapp/lib/src/foo.dart"

        // Resolve to a project-relative path, skipping framework paths.
        let rel = if let Some(stripped) = raw_path.strip_prefix("package:") {
            let slash = stripped.find('/')?;
            let pkg = &stripped[..slash];
            if FRAMEWORK_PACKAGES.contains(&pkg) {
                // Skip — advance past this ".dart:" occurrence and keep looking.
                search = &search[pos + 6..];
                continue;
            }
            // User package: keep the path after "package:name/"
            stripped[slash + 1..].to_string()
        } else if raw_path.starts_with("lib/")
            || raw_path.starts_with("test/")
            || raw_path.starts_with("bin/")
            || raw_path.starts_with("integration_test/")
        {
            raw_path.to_string()
        } else {
            search = &search[pos + 6..];
            continue;
        };

        // Now parse ":LINE[:COL]" that follows ".dart:"
        let after = &search[pos + 6..]; // text after the first ":"
        let line = parse_leading_u32(after)?;
        let col = after
            .find(':')
            .filter(|&j| after[j + 1..].starts_with(|c: char| c.is_ascii_digit()))
            .and_then(|j| parse_leading_u32(&after[j + 1..]));

        return Some(FileRef { rel_path: rel, line, col });
    }

    None
}

/// Parse the leading decimal digits of `s` as a `u32`. Returns `None` if the
/// string doesn't start with a digit.
fn parse_leading_u32(s: &str) -> Option<u32> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    s[..end].parse().ok()
}

// ── IDE opener ───────────────────────────────────────────────────────────

/// Open a source file at a specific location in the given IDE.
///
/// `project_root` must be an absolute path to the Flutter project root
/// (typically `std::env::current_dir()` from where flutter-cli was invoked).
///
/// Returns a short status string suitable for the TUI banner —
/// success messages name the IDE, error messages suggest a fix.
pub fn open(ide: IdeKind, project_root: &Path, file_ref: &FileRef) -> String {
    let abs = project_root.join(&file_ref.rel_path);
    let line = file_ref.line;
    let col = file_ref.col.unwrap_or(1);
    match ide {
        IdeKind::VsCode => open_vscode(&abs.to_string_lossy(), line, col),
        IdeKind::AndroidStudio => open_android_studio(&abs.to_string_lossy(), line, col),
    }
}

fn open_vscode(abs_path: &str, line: u32, col: u32) -> String {
    // `code --goto FILE:LINE:COL` — accepted by VS Code, Cursor, Windsurf,
    // VSCodium. We try each binary in order, stopping at the first success.
    let target = format!("{abs_path}:{line}:{col}");
    for bin in &["code", "cursor", "codium", "windsurf"] {
        let ok = std::process::Command::new(bin)
            .args(["--goto", &target])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok();
        if ok {
            return format!("Opened {}:{line} in VS Code", shorten(abs_path));
        }
    }
    "`code` binary not found — is VS Code installed and on PATH?".to_string()
}

fn open_android_studio(abs_path: &str, line: u32, col: u32) -> String {
    // We previously sent a raw HTTP/1.0 request to
    // `/api/file?file=...&line=...` at 127.0.0.1:63342 — the legacy
    // JetBrains REST endpoint. That endpoint 404s on Android Studio
    // Narwhal 4 / 2025.1.4 (verified empirically; the path was renamed /
    // redirected to `/file/` which strips the query string). The
    // current supported entry point is the `idea://` URL scheme:
    // LaunchServices binds it to Android Studio, and the
    // `CommandLineProcessor` accepts `idea://open?file=ABS&line=N&column=N`
    // (visible in `idea.log` as `external URI request: idea://open?...`).
    //
    // So we just shell out to macOS's `open` with the right URL. Same
    // mechanism the user could trigger by typing
    // `open "idea://open?file=..."` themselves.
    let url = format!(
        "idea://open?file={}&line={}&column={}",
        url_encode(abs_path),
        line,
        col
    );
    let status = std::process::Command::new("open")
        .arg(&url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => format!("Opened {}:{line} in Android Studio", shorten(abs_path)),
        Ok(_) => "Android Studio: `open` exited non-zero — is the `idea:` URL scheme registered?"
            .to_string(),
        Err(_) => "Android Studio: failed to spawn `open` — is macOS LaunchServices reachable?"
            .to_string(),
    }
}

/// Shorten an absolute path for display in the TUI banner: keep only the
/// portion starting at `lib/`, `test/`, etc. for readability.
fn shorten(abs: &str) -> &str {
    for prefix in &["lib/", "test/", "bin/", "integration_test/"] {
        if let Some(pos) = abs.find(prefix) {
            return &abs[pos..];
        }
    }
    abs
}

/// Minimal percent-encoding: encode characters that are not safe in a
/// query-string value. We keep `/` and `:` unencoded because they are
/// part of the path and port, which Android Studio expects unescaped.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'/'
            | b':'
            | b'@' => out.push(b as char),
            b' ' => out.push_str("%20"),
            _ => {
                out.push('%');
                out.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0').to_ascii_uppercase());
                out.push(char::from_digit((b & 0xf) as u32, 16).unwrap_or('0').to_ascii_uppercase());
            }
        }
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_lib_path() {
        let msg = "#0      MyWidget.build (lib/src/home.dart:42:13)";
        let r = extract_file_ref(msg).unwrap();
        assert_eq!(r.rel_path, "lib/src/home.dart");
        assert_eq!(r.line, 42);
        assert_eq!(r.col, Some(13));
    }

    #[test]
    fn extract_package_user_path() {
        let msg = "#0      MyPage._build (package:myapp/lib/src/page.dart:100:5)";
        let r = extract_file_ref(msg).unwrap();
        assert_eq!(r.rel_path, "lib/src/page.dart");
        assert_eq!(r.line, 100);
        assert_eq!(r.col, Some(5));
    }

    #[test]
    fn skip_flutter_framework_path() {
        let msg = "#1      StatefulElement.build (package:flutter/src/widgets/framework.dart:4865:27)";
        assert!(extract_file_ref(msg).is_none());
    }

    #[test]
    fn skip_dart_framework_path() {
        let msg = "#2      Object.noSuchMethod (package:dart/core/object.dart:10:1)";
        assert!(extract_file_ref(msg).is_none());
    }

    #[test]
    fn picks_user_frame_over_framework() {
        // Stack trace with a framework frame first, user frame second.
        let msg = concat!(
            "#0 StatefulElement.build (package:flutter/src/widgets/framework.dart:4865:27)\n",
            "#1 MyWidget._build (package:myapp/lib/src/widget.dart:12:3)",
        );
        let r = extract_file_ref(msg).unwrap();
        assert_eq!(r.rel_path, "lib/src/widget.dart");
        assert_eq!(r.line, 12);
    }

    #[test]
    fn no_file_ref_returns_none() {
        assert!(extract_file_ref("plain log line without any dart path").is_none());
    }

    #[test]
    fn parse_leading_u32_works() {
        assert_eq!(parse_leading_u32("42:13)"), Some(42));
        assert_eq!(parse_leading_u32("100"), Some(100));
        assert_eq!(parse_leading_u32(""), None);
        assert_eq!(parse_leading_u32("abc"), None);
    }

    #[test]
    fn url_encode_spaces() {
        assert!(url_encode("/Users/foo bar/lib/main.dart").contains("%20"));
    }

    #[test]
    fn integration_test_path_recognised() {
        let msg = "#0      main (integration_test/app_test.dart:15:3)";
        let r = extract_file_ref(msg).unwrap();
        assert_eq!(r.rel_path, "integration_test/app_test.dart");
        assert_eq!(r.line, 15);
    }

}
