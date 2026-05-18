# `fl` — A modern Flutter CLI with seamless USB↔WiFi continuity

**Status:** Approved design — MVP 1
**Date:** 2026-05-18
**Platform:** macOS Apple Silicon
**Language:** Rust (edition 2021, MSRV 1.75)

## 1. Goal

Build `fl`, a wrapper around the Flutter SDK that offers:

1. A modern terminal UI (ratatui-based dashboard) with colors, animations, shimmer effects and ASCII art at the level of Claude Code CLI.
2. A **seamless USB→WiFi handover** so that unplugging a real device during `flutter run` does not break the hot-reload session — WiFi takes over without dropping VM Service state.
3. Direct VM Service integration for faster hot reload and richer live metrics (FPS, memory, widget rebuilds) than the standard Flutter CLI surfaces.

This document covers **MVP 1** only: `fl run`, `fl devices`, and the ADB WiFi failover machinery. `fl build`, `fl test`, `fl pub`, `fl doctor`, `fl clean` are explicitly out of scope for MVP 1 and will be planned in later specs.

## 2. Non-goals

- Replacing or reimplementing the Flutter tool — `fl` is a wrapper.
- Cross-platform support beyond macOS Apple Silicon (Intel/Linux/Windows are post-MVP).
- iOS device support in MVP 1 (Android only — iOS does not have the same USB↔WiFi switching story).
- Distribution via Homebrew tap — MVP 1 ships via `cargo install --path fl-cli` only.

## 3. Stack

| Concern | Choice |
|---|---|
| Language | Rust 1.75+ |
| TUI | `ratatui` + `crossterm` |
| Async runtime | `tokio` (multi-thread) |
| CLI parsing | `clap` v4 (derive) |
| WebSocket (VM Service) | `tokio-tungstenite` |
| JSON | `serde` + `serde_json` |
| Logging | `tracing` + `tracing-appender` (file only, never stdout) |
| Error handling | `anyhow` at boundaries, `thiserror` inside crates |
| Tests | `cargo test`, `insta` for snapshot, faux binaries for integration |

## 4. Workspace layout

Cargo workspace with six crates. Each crate has a single purpose, communicates with the rest only through an `AppEvent` channel, and is unit-testable without devices.

```
fl/
├── Cargo.toml              # workspace
├── crates/
│   ├── fl-cli/             # binary entrypoint, clap parsing, sub-command dispatch
│   ├── fl-core/            # AppEvent enum, Config, shared types
│   ├── fl-tui/             # ratatui rendering, theme, animations, layout
│   ├── fl-flutter/         # spawn `flutter --machine`, parse JSON daemon protocol
│   ├── fl-vmservice/       # WebSocket client for Dart VM Service JSON-RPC
│   └── fl-adb/             # device detection, pre-pairing, watcher, IP discovery
└── docs/superpowers/specs/
```

### Inter-crate communication

A single `tokio::sync::mpsc::Sender<AppEvent>` is passed to every long-running task. The TUI is the only consumer. No crate depends on another except through `fl-core`.

```rust
// fl-core
pub enum AppEvent {
    Device(DeviceEvent),       // from fl-adb
    Flutter(FlutterEvent),     // from fl-flutter
    Vm(VmEvent),               // from fl-vmservice
    Key(KeyEvent),             // from crossterm event reader
    Tick,                      // 33ms render tick
}
```

## 5. The handover trick

The classical approach — unplug the cable, then attempt to migrate the session — is fragile and racey. `fl` inverts the problem: **the VM Service session runs over TCP/IP from second one**. USB is used only to pair and install the APK. The cable can be removed at any moment without anything to migrate.

Sequence at `fl run` startup:

1. `fl-adb` runs `adb devices -l` → finds a USB device (serial `ABC123`).
2. In parallel (≤ 1.5 s total):
   - `adb -s ABC123 tcpip 5555` — enables network adb on the phone.
   - `adb -s ABC123 shell ip -f inet addr show wlan0` — obtains the phone's WiFi IP.
   - `adb connect 192.168.1.42:5555` — registers a second serial for the same device, over WiFi.
3. `fl-flutter` spawns: `flutter run --machine -d 192.168.1.42:5555 …`.
4. The daemon emits `app.started` with `vmServiceUri = ws://127.0.0.1:<port>/<token>/ws`. (Flutter sets up an ADB-forwarded port; over WiFi, that forward is a TCP relay through the network connection, not the USB tunnel.)
5. `fl-vmservice` connects, subscribes to streams `Stdout`, `Stderr`, `Isolate`, `GC`, `Extension`.
6. The TUI starts rendering at 30 fps.

**Watcher:** `fl-adb` runs `adb track-devices` (binary streaming protocol on `localhost:5037`, not polling). When the USB serial disappears, an `AppEvent::Device(DeviceEvent::UsbDisconnected)` is emitted → the TUI shows a transient banner; no other component reacts because the VM Service connection is independent of USB.

### Failure modes for the handover

| Situation | Behaviour |
|---|---|
| Cannot extract IP (no WiFi, mobile data only) | Fall back to USB-only mode. Banner: "Mode USB seul — débrancher coupera la session." |
| `adb connect` fails (port blocked, firewall) | Same fallback as above. |
| WiFi drops mid-session | Exponential backoff `adb connect` (1s, 2s, 4s, 8s, capped at 30s). Banner orange "Reconnecting…". Queued reload requests retained. |
| Device IP changes (DHCP renew) | Discover new IP via mDNS service `_adb-tls-connect._tcp` (Android 11+) or via the USB tunnel if cable is plugged in. |
| VM Service WebSocket closes unexpectedly | Auto-reconnect, re-subscribe streams, re-apply active overrides (theme/paint/platform). |

## 6. `fl run` — what the user sees

### Splash (≤ 1 s)

Big ASCII art of "fl" (block characters, 6 rows tall) rendered with a horizontal shimmer: each column linearly interpolates between two theme colors based on `(t - t0) / 800ms`, sweeping left to right once, then settling on the accent color.

### Dashboard

```
╭─ fl ── my_app · debug · Pixel 8 ───────────────────────────────────╮
│                                                                     │
│ ╭─ Logs ──────────────────────────╮ ╭─ Performance ───────────────╮ │
│ │ INFO  App started               │ │ FPS    ▁▂▃▅▆▇█▇▆ 60.0       │ │
│ │ DEBUG HomePage build            │ │ Frame  16.6ms  raster 4.2ms │ │
│ │ WARN  Image cache full          │ │ Memory 142 MB  ▁▂▄▆         │ │
│ │ ▍                               │ │ Rebuilds 12/s               │ │
│ ╰─────────────────────────────────╯ ╰─────────────────────────────╯ │
│                                     ╭─ Devices ───────────────────╮ │
│                                     │ ● Pixel 8     🔗 WiFi   ✓   │ │
│                                     │   192.168.1.42:5555         │ │
│                                     │ ○ USB backup  ⚡ ready       │ │
│                                     ╰─────────────────────────────╯ │
╰─────────────────────────────────────────────────────────────────────╯
 [r] reload  [R] restart  [b] theme  [p] paint  [o] platform  [w] wifi  [q] quit
```

The header shows app name, build mode, and the active device. No clock and no WiFi detail in the header (the IP is shown only in the Devices panel, on purpose, to keep the header calm).

### Panels

- **Logs** — scrollable buffer (default 5,000 lines, ring-buffered). Coloring per level: INFO cyan, WARN yellow, ERROR red, DEBUG dim gray, stdout default. `/` opens a filter prompt; `c` clears.
- **Performance** — sparkline FPS over the last 60 frames (green ≥ 55, yellow ≥ 30, red below), frame budget (UI + raster split when available from `Timeline` extension), memory sparkline, rebuilds-per-second counter from the widget inspector extension.
- **Devices** — list of known devices with a colored bullet (filled = active session, hollow = standby/backup). USB↔WiFi transitions animate via a 600 ms color fade orange → green.
- **Footer** — context-sensitive shortcuts. Defaults shown above; in filter mode shows the filter prompt instead.

### Theme

Tokyo Night palette (24-bit when `COLORTERM=truecolor`, 256-color fallback otherwise):

- background `#1a1b26`, text `#c0caf5`
- accent `#7aa2f7`, success `#9ece6a`, warn `#e0af68`, error `#f7768e`, dim `#565f89`

### Animations

| Element | Technique |
|---|---|
| Splash shimmer | Per-column lerp between dim and accent, single sweep over 800 ms |
| Spinners | Braille pattern `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`, 80 ms per frame |
| Progress bars | Sub-character resolution via half-blocks `▏▎▍▌▋▊▉█`, gradient accent→cyan |
| USB→WiFi transition | Color fade on the Devices panel border, 600 ms |
| Hot reload success | Header flashes success-green for 200 ms |
| Render loop | 30 fps target (33 ms tick), dirty-flag based — render only on state change. Animations interpolate against wall-clock delta to stay smooth regardless of tick jitter. |

### Keyboard

Flutter passthroughs (forwarded as VM Service extension calls, not via `flutter` stdin):

| Key | Action | VM Service call |
|---|---|---|
| `r` | Hot reload | `s0.reloadSources` |
| `R` | Hot restart | `ext.flutter.hotRestart` (via `s0.callServiceExtension`) |
| `b` | Toggle brightness (dark/light) | `ext.flutter.brightnessOverride` |
| `p` | Toggle debug paint | `ext.flutter.debugPaint` |
| `o` | Toggle platform (iOS/Android) | `ext.flutter.platformOverride` |
| `P` | Toggle performance overlay | `ext.flutter.showPerformanceOverlay` |

`fl`-native:

| Key | Action |
|---|---|
| `w` | Force USB↔WiFi switch on the current device |
| `/` | Open log filter prompt |
| `c` | Clear logs panel |
| `?` | Help overlay |
| `Ctrl+L` | Force redraw |
| `q` / `Ctrl+C` | Quit (sends `app.stop` to Flutter daemon, then exits) |

## 7. `fl devices`

Non-interactive list. Prints a table with:

| Column | Source |
|---|---|
| Status | `adb track-devices` snapshot |
| Name | `adb -s … shell getprop ro.product.model` |
| Serial | from `adb devices -l` |
| Connection | USB / WiFi (inferred from serial format `<ip>:<port>` vs alphanumeric) |
| IP | `adb shell ip -f inet addr show wlan0`, cached |
| Battery | `adb shell dumpsys battery` |
| Android | `adb shell getprop ro.build.version.release` |

Output uses the same Tokyo Night colors and box-drawing characters as the dashboard. No interactive mode for MVP 1.

## 8. Error handling

Errors surface as TUI banners (full-screen overlay for fatal, top banner for recoverable). Stack traces and full context land in `~/.cache/fl/fl.log` only.

| Situation | Behaviour |
|---|---|
| `flutter` not found | Probe `~/fvm/default/bin/flutter`, `~/development/flutter/bin/flutter`, `$FLUTTER_ROOT/bin/flutter`. If none, error overlay with install link and exit 1. |
| `adb` not found | Probe `$ANDROID_HOME/platform-tools/adb`, `~/Library/Android/sdk/platform-tools/adb`. Error overlay if absent. |
| No device | Waiting screen "En attente d'un appareil…" with animated spinner, live device list. `q` to quit. |
| Pre-pairing fails | Banner, USB-only fallback, session continues. |
| WiFi drops | Banner orange "Reconnecting…", exponential backoff, no exit. |
| `flutter run` crashes | Capture stderr, error panel with stack, `[Enter] relancer`. |
| VM Service WS closes | Auto-reconnect, re-subscribe, re-apply overrides. |
| Daemon emits `daemon.logMessage level=error` | Promote to ERROR in Logs panel (red). |

## 9. Testing strategy

### Unit tests

Each crate has its own `tests/` directory, runnable in isolation without devices.

- **`fl-adb`** — parse `adb devices -l` output, parse `ip addr` output, decode `adb track-devices` binary frames. Fixtures captured from real devices stored in `crates/fl-adb/tests/fixtures/`.
- **`fl-flutter`** — parse the `--machine` JSON event stream. Fixtures captured by running real `flutter run --machine` against the sample project, stored as `.jsonl`.
- **`fl-vmservice`** — mock WebSocket server using `tokio-tungstenite` server side. Verifies the JSON-RPC envelope, stream subscription, `reloadSources` round-trip, extension calls.
- **`fl-tui`** — snapshot tests of the ratatui `Buffer` after rendering known state. `insta` golden files. Catches accidental layout shifts.
- **`fl-core`** — round-trip serialization of `Config`, `AppEvent` variant exhaustiveness.

### Integration tests

A faux `adb` and faux `flutter` are shipped as shell scripts under `tests/fixtures/bin/` and prepended to `PATH` for the test process. They replay scripted scenarios:

1. Nominal: device appears USB → pre-pairing → flutter starts → hot reload → quit.
2. Unplug mid-session: device disappears from `adb track-devices` → TUI banner → session still works.
3. WiFi drops: faux ADB starts refusing connections → backoff → recovery.
4. Device IP changes: faux ADB advertises new IP via mDNS fixture.

Integration tests run the binary in a "headless" mode (env `FL_HEADLESS=1`) which replaces the TUI with a deterministic event-log dump, so assertions compare event sequences rather than rendered frames.

### Manual smoke test

Tracked in `docs/SMOKE.md` (post-implementation): real Pixel/Samsung phone, real network, real Flutter sample app. Checklist covers unplug/replug, hot reload, every sub-key, WiFi off/on, app crash, USB-only fallback.

## 10. Distribution & logging

- **Install (MVP):** `cargo install --path crates/fl-cli`.
- **Internal logs:** `~/.cache/fl/fl.log`, rotated at 10 MB, retain 3 files. Level via `FL_LOG=debug` (default `info`). `tracing` exclusively — `println!` is banned outside of `fl-cli::main` startup messages.
- **Config:** `~/.config/fl/config.toml`, optional, for theme overrides and default flags. Absent file = built-in defaults.

## 11. Open assumptions

These are decisions made for MVP 1 that should be revisited before scope expands:

1. **Android only.** iOS over WiFi requires a different path (`xcrun devicectl`, Network.framework). Out of scope until proven necessary.
2. **One device at a time.** Multi-device sessions add multiplexing complexity to the VM Service layer. The Devices panel will *display* multiple, but the active session is single.
3. **No remote debugging.** Phone must be on the same LAN as the Mac.
4. **No profile/release modes.** `fl run` is debug only for MVP 1.

## 12. Out of scope (for later specs)

`fl build`, `fl test`, `fl pub`, `fl doctor`, `fl clean`, iOS support, multi-device sessions, profile/release modes, Homebrew tap, telemetry, plugin system, cloud devices.
