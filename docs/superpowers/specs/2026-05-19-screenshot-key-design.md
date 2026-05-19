# `s` — Capture screenshots from the multi-device dashboard

**Date:** 2026-05-19
**Status:** Approved (pending implementation plan)

## Summary

Add an `s` keybinding to the `flutter run` TUI that captures a PNG of every connected device's currently-rendered frame and writes the files into `./screenshots/` at the Flutter project root. Mirrors the existing `b`/`p`/`o`/`P` extension-key flow: TUI shows a banner, `multi.rs` fans the request out to every live VM Service in parallel, results are aggregated into one banner.

## Motivation

The other in-TUI shortcuts (`r`, `R`, `b`, `p`, `o`, `P`, `/`, `c`) cover the inner-loop tasks that interrupt flow when you have to leave the terminal. Screenshots are the missing one — today the user has to alt-tab to Simulator.app / Android Studio / a physical device, take the shot manually, then drag the file out. With several devices on screen at once, that's N round-trips for the same frame. One `s` keystroke produces the whole set, named and dated, ready to drop into a PR description.

## User-facing behaviour

When the user presses `s` in the multi-device dashboard:

1. For each device whose VM Service is up, a PNG of the current frame is written to `<project-root>/screenshots/screenshot-<short-name>-<YYYYMMDD-HHMMSS>.png`.
2. The `screenshots/` directory is created if it doesn't exist (`fs::create_dir_all`).
3. The banner reflects the outcome:
   - All devices succeeded, N ≥ 2 → green banner `📸 Saved N → screenshots/`
   - Exactly one device, success → green banner `📸 screenshots/<filename>`
   - At least one failure → red banner `Screenshot failed (<short-name>): <reason>` per failing device, plus the success banner for the rest
4. If no device has a VM Service yet (early in build/install), the key is dropped silently — same rule as `b`/`p`/`o` ([multi.rs:617-624](../../../crates/fl-cli/src/multi.rs)).
5. If the user is in filter-input mode (`/` was pressed and `Enter` not yet hit), `s` is appended to the filter buffer like any other character — no special-casing.

The timestamp uses the local clock, second precision. Filename short-names already exist on each session (`Session::short_name`). The implementation plan must verify they are safe to embed in a filename as-is; if not, a small sanitiser (strip whitespace / path separators, lowercase) goes in alongside the new keybinding.

## Architecture

Three crates are touched. The change is additive — no existing behaviour moves.

### 1. `fl-vmservice` — new VM Service wrapper

`crates/fl-vmservice/src/ext.rs`:

```rust
/// Capture the current frame of the running app and return the raw PNG bytes.
/// Uses the Dart VM Service's `_flutter.screenshot` RPC, which returns
/// `{ "type": "Screenshot", "screenshot": "<base64 png>" }`.
pub async fn screenshot_png(&self, isolate_id: &str) -> anyhow::Result<Vec<u8>>
```

Implementation: call `_flutter.screenshot` with `{ "isolateId": isolate_id }`, pull the `screenshot` field as a string, base64-decode it (engine framework — `base64` crate, already in the dep tree via other crates; if not, add it as a direct dep of `fl-vmservice`). Returns `Err` for missing field, malformed base64, or upstream VM Service errors.

The wrapper does NOT touch the filesystem — it just returns bytes. That keeps it symmetric with `toggle_brightness` & friends, and makes it unit-testable against the same `spawn_mock_handler` pattern as the other ext tests.

### 2. `fl-tui` — keybinding arm

`crates/fl-tui/src/app.rs`, in the main `match key` block around line 567:

```rust
fl_core::KeyEvent::Char('s') => {
    self.show_banner(BannerKind::Info, "📸 Capturing…");
}
```

The arm intentionally does no work beyond the optimistic banner — the key flows through the existing extension-key forwarding channel to `multi.rs`, exactly as `b`/`p`/`o` already do. `multi.rs` will replace this banner with the success/failure banner once the captures complete.

### 3. `fl-cli` — fan-out and disk write

`crates/fl-cli/src/multi.rs`, in the `match key_copy` block around line 652:

```rust
FlKey::Char('s') => {
    let bytes = match client.screenshot_png(&iso).await {
        Ok(b) => b,
        Err(e) => return Some((short, Err(e))),
    };
    let dir = std::path::PathBuf::from("screenshots");
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return Some((short, Err(e.into())));
    }
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("screenshot-{short}-{ts}.png"));
    match tokio::fs::write(&path, &bytes).await {
        Ok(()) => return Some((short.clone(), Ok(serde_json::json!({ "path": path })))),
        Err(e) => return Some((short, Err(e.into()))),
    }
}
```

(Final form may differ slightly to fit the surrounding closure shape; the structure is what matters: VM call → mkdir → write → return per-device result.)

The post-`join_all` result aggregator (already in `multi.rs` around lines 663-…) gains a new `FlKey::Char('s')` arm in its label match: on success, count successes and emit one banner; on partial failure, emit individual failure banners plus the aggregated success banner.

Two cwd considerations:
- `multi.rs` is already executed from the Flutter project root (that's where `flutter run` resolves devices/pubspec). `screenshots/` is therefore project-rooted by default — same convention as `coverage/` from `flutter test --coverage`.
- We don't add `screenshots/` to `.gitignore` ourselves — that's a user decision. The README mentions the path so the user can decide.

### 4. README

`README.md` keybinding table (lines 87-98):

```markdown
| `s` | screenshot every device → `./screenshots/` |
```

Inserted after `| o | toggle platform (iOS / Android) |` and before `| / | filter logs live |` so it lives with the other "act on the running app" keys, not with the log-management ones.

## Data flow

```
key 's'
  → fl-tui/app.rs: show "📸 Capturing…" banner
  → channel → fl-cli/multi.rs: fan out per session
      ├─ session A: client.screenshot_png(iso) → mkdir → write → Ok(path)
      ├─ session B: client.screenshot_png(iso) → Err(timeout)
      └─ session C: client.screenshot_png(iso) → mkdir → write → Ok(path)
  → join_all
  → aggregator emits banners:
      "📸 Saved 2 → screenshots/"
      "Screenshot failed (sessionB): VM Service timeout"
```

## Error handling

| Failure | Handling |
|---|---|
| No VM Service yet (early in build) | Drop the key silently, same as `b`/`p`/`o` |
| `_flutter.screenshot` returns `Sentinel` | Treat as failure, red banner per device — same pattern already in [multi.rs:672-676](../../../crates/fl-cli/src/multi.rs) |
| Base64 decode fails | Red banner per device with decode error |
| `create_dir_all` fails (permissions, conflicting file) | Red banner per device; the call is cheap and idempotent so we don't pre-check before fan-out |
| `tokio::fs::write` fails | Red banner per device |
| All devices fail | N red banners, no success banner |

## Testing

- **Unit test in `fl-vmservice/src/ext.rs`**, mirroring `hot_reload_calls_reload_sources` (the pattern at line 134-145):
  - Mock handler asserts `req["method"] == "_flutter.screenshot"` and `req["params"]["isolateId"] == "isolates/1"`
  - Returns `{ "type": "Screenshot", "screenshot": "<base64 of \x89PNG\r\n\x1a\n>" }`
  - Assert returned `Vec<u8>` starts with the PNG magic bytes
- No TUI test for the key arm — consistent with `b`/`p`/`o` which aren't unit-tested either; coverage there is by manual smoke.
- Manual smoke test: `flutter run` against the example app with 1 device, press `s`, confirm a PNG appears in `screenshots/` and opens correctly. Then with 2 devices, confirm both files appear with distinct names.

## Out of scope (YAGNI)

- Formats other than PNG (raw / JPEG).
- Copy to clipboard.
- Auto-open after save (`open` / `xdg-open`).
- Configurable output directory (CLI flag or env var).
- Per-device selection (single device only, picker after `s`).
- Capturing the TUI itself.
- Pre-creating `.gitignore` entry for `screenshots/`.

If any of these come up later they can be added incrementally without revisiting this spec.

## Open questions

None. Behaviour, file layout, and error model are agreed.
