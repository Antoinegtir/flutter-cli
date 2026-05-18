# Failover hardening — WiFi reconnect & mDNS IP discovery

**Status:** Approved design — Sub-project A (post-MVP 1)
**Date:** 2026-05-18
**Depends on:** [MVP 1 design](2026-05-18-fl-cli-design.md) and the shipped MVP 1 implementation
**Out of scope:** USB-on-replug switching (separate concern), multi-device (sub-project C), iOS (sub-project D), profile/release modes (sub-project B)

## 1. Goal

Make the active WiFi session of `fl run` survive every network hiccup short of the phone being powered off:

1. If the WiFi serial disappears (network drop, sleep, etc.) → exponential-backoff `adb connect`, forever.
2. If the device gets a new IP (DHCP renew, network switch) → discover it via mDNS, re-target, reconnect.

The user should be able to walk between rooms, toggle airplane mode, switch WiFi networks (on the same LAN), and never lose hot reload state.

## 2. Stack additions

- `mdns-sd = "0.11"` (active fork, pure-Rust, async-friendly via channel API)

No other new dependencies.

## 3. New module layout

```
crates/fl-adb/src/
├── reconnect.rs        # ReconnectManager + state machine + tests
└── mdns.rs             # adb-tls-connect/adb listener + parser + tests
```

Both modules expose a `pub async fn run(...)` that consumes input events, emits output events through `tokio::sync::mpsc::Sender<DeviceEvent>`, and never returns under normal operation (long-running task).

## 4. ReconnectManager — state machine

```rust
// crates/fl-adb/src/reconnect.rs
pub enum State {
    Attached { target: WifiTarget, device_name: String },
    Reconnecting { target: WifiTarget, attempt: u32 },
}
```

### Transitions

| From → To | Trigger | Side effect |
|---|---|---|
| Attached → Reconnecting | `DeviceEvent::Lost{serial}` where `serial == target.serial()`, debounced 500 ms | Set `attempt = 0`, emit `WifiReconnecting{attempt: 0}`, schedule first connect in `delay(0) = 1 s` |
| Reconnecting → Reconnecting | Backoff tick reaches `delay(attempt)` and `adb connect` still fails | `attempt += 1`, emit `WifiReconnecting{attempt}` |
| Reconnecting → Attached | `adb connect` succeeds OR `DeviceEvent::Discovered{d}` arrives with matching serial | Emit `WifiReconnected`, reset attempt |
| Attached → Attached | `IpChanged{new_ip}` from mDNS | Update `target.ip`, emit `IpChanged` |
| Reconnecting → Reconnecting (target update) | `IpChanged{new_ip}` from mDNS | Update `target.ip`, reset backoff timer to 0 (try new target immediately), keep attempt counter |

### Backoff

`delay(attempt) = min(2u64.pow(attempt), 30)` seconds, starting at `attempt = 0`. Sequence: 1, 2, 4, 8, 16, 30, 30, 30…

The Reconnecting task wakes itself with `tokio::time::sleep(Duration::from_secs(delay))` and then attempts `adb connect <ip>:<port>` via `CommandRunner`. Each failed attempt increments the counter.

### Debounce

A `Lost` event for the target serial is held for 500 ms before transitioning to Reconnecting. If a matching `Discovered` arrives within that window, we cancel and stay in Attached. This absorbs the brief flap that ADB occasionally produces around state changes.

## 5. mDNS listener

### Service types

Subscribe to both:
- `_adb-tls-connect._tcp.local.` (Android 11+ wireless debugging — pairs over TLS)
- `_adb._tcp.local.` (older Android, some configurations)

### Filtering

Each `ServiceResolved` event carries:
- `fullname`: e.g. `adb-RFCM5028XBR._adb-tls-connect._tcp.local.`
- `addresses`: `HashSet<IpAddr>`
- `port`: u16
- `properties`: HashMap (may include `name=Pixel 8`)

We filter on **device name** stored in `ReconnectManager` (resolved at startup via `adb -s <serial> shell getprop ro.product.model`). If the resolved service name or `name` property matches the stored device name, take the first non-loopback IPv4 address from `addresses`.

If the discovered IP differs from `target.ip`, emit `DeviceEvent::IpChanged { serial: target.serial(), old_ip, new_ip }` and update the target inside the manager.

### Reliability

mDNS is best-effort. Loss of mDNS does not affect reconnect — backoff continues against the last-known IP. mDNS only opportunistically improves recovery time when the IP changes.

## 6. Type changes

### `fl-core/src/events.rs`

Add a variant to `DeviceEvent`:

```rust
pub enum DeviceEvent {
    // ...existing variants
    IpChanged {
        serial: String,
        old_ip: String,
        new_ip: String,
    },
}
```

`WifiReconnecting { attempt: u32 }` and `WifiReconnected` are already defined — they finally have producers.

### `fl-tui/src/app.rs`

Extend `Banner`:

```rust
pub struct Banner {
    pub kind: BannerKind,
    pub message: String,
    pub shown_at: Instant,
    pub duration: Option<Duration>,  // None = persistent until cleared
}
```

`expire_banner` ignores banners whose `duration` is `None`. Set persistent banners explicitly via `show_persistent_banner`, regular ones via `show_banner` (unchanged).

Apply rules:
- `WifiReconnecting { attempt }` → `show_persistent_banner(Warn, format!("Reconnecting WiFi (#{attempt})"))`
- `WifiReconnected` → clear persistent banner if any, `show_banner(Success, "WiFi reconnected", 3 s)`
- `IpChanged { new_ip, .. }` → `show_banner(Success, format!("New IP: {new_ip}"), 3 s)`

### `fl-tui/src/panels/devices.rs`

When `state.banner` is persistent and the wording starts with `Reconnecting`, add a third line under the active device: `↻ Reconnecting (attempt N)` styled dim. This duplicates the banner info inside the panel for clarity (the banner is at the top of the screen, the panel at the bottom-right).

## 7. Wiring in `fl run`

In `crates/fl-cli/src/run_cmd.rs`, after the initial `pre_pair_wifi` succeeds, spawn two new tokio tasks before the TUI loop:

```rust
// Pseudo, real code in the plan:
let target = WifiTarget { ip, port };
let device_name = runner.run("adb", &["-s", &usb_serial, "shell", "getprop", "ro.product.model"])
    .await?.stdout.trim().to_string();

let (reconnect_tx, _reconnect_handle) = ReconnectManager::spawn(
    target.clone(), device_name.clone(), runner.clone(),
    device_event_subscription, event_tx.clone(),
);
let _mdns_handle = mdns::spawn_listener(device_name, event_tx.clone());
```

Both tasks share the same `event_tx` channel the TUI already drains. No new infrastructure.

For now, `device_event_subscription` is a clone of the existing track-devices `mpsc::Receiver<DeviceEvent>`. We add a fan-out: the existing watcher task sends each event to BOTH the TUI channel AND the ReconnectManager channel.

## 8. Error handling

| Situation | Behaviour |
|---|---|
| `adb connect` returns "failed to connect" | Counted as a failed attempt, backoff continues |
| `adb connect` times out at process level | Same as above |
| mDNS browser fails to start (no network interface) | Warn-log, mDNS feature silently disabled, reconnect still works |
| mDNS announces an IP that immediately fails to connect | Don't roll back target; keep the new IP and let backoff retry |
| Device name lookup fails at startup (USB already pulled before resolution) | Use `target.ip` as identifier — mDNS filtering becomes IP-based instead of name-based |
| User presses `w` (manual WiFi switch) while in Reconnecting | Force an immediate connect attempt against current target; do not reset attempt counter |

## 9. Testing strategy

### Unit (`fl-adb/src/reconnect.rs`)

- Backoff formula: `delay(0) == 1`, `delay(4) == 16`, `delay(5) == 30`, `delay(20) == 30`
- Lost → Reconnecting transition after 500 ms debounce
- Discovered within debounce cancels the transition
- mDNS IpChanged in Reconnecting state resets backoff timer (verified by a short fake clock)
- Manual `w` key: ForceReconnect input triggers an immediate attempt

### Unit (`fl-adb/src/mdns.rs`)

- Parse a `ServiceResolved` fixture and extract IPv4
- Reject services whose name doesn't match the target device name
- Pick the first non-loopback IPv4 when multiple addresses present

Use a hand-built `ServiceEvent` value rather than running a real mDNS server in tests. The library exposes constructors.

### Integration

Extend `tests/fixtures/`:

- `tests/fixtures/scenarios/wifi_drop.txt`: simulates the WiFi serial disappearing from `adb devices` mid-session. Faux `adb`'s `connect` succeeds on the third call.
- `tests/fixtures/bin/adb`: add support for `FL_ADB_CONNECT_FAILS_FIRST_N=2` env var so the script returns "failed to connect" the first 2 times.

New headless integration test `headless_wifi_drop` asserts the event sequence:

```
WifiPaired
... (running)
Lost or UsbDisconnected
WifiReconnecting attempt: 1
WifiReconnecting attempt: 2
WifiReconnected
```

### Manual

Real Pixel + real router. Toggle airplane mode for 10 seconds. Verify session continues. Then change WiFi networks (with phone). Verify mDNS picks up new IP and session resumes.

## 10. File-level diff summary

| File | Change |
|---|---|
| `crates/fl-adb/Cargo.toml` | Add `mdns-sd = "0.11"` |
| `crates/fl-adb/src/lib.rs` | `pub mod mdns; pub mod reconnect;` + re-exports |
| `crates/fl-adb/src/reconnect.rs` | **new** — ReconnectManager |
| `crates/fl-adb/src/mdns.rs` | **new** — mDNS listener |
| `crates/fl-core/src/events.rs` | Add `DeviceEvent::IpChanged` |
| `crates/fl-tui/src/app.rs` | `Banner::duration: Option<Duration>` + persistent banner helper + new event handling |
| `crates/fl-tui/src/panels/devices.rs` | Show reconnecting indicator under active device |
| `crates/fl-cli/src/run_cmd.rs` | Spawn ReconnectManager + mDNS listener after pre-pair; fan-out track-devices events |
| `tests/fixtures/bin/adb` | Honor `FL_ADB_CONNECT_FAILS_FIRST_N` |
| `tests/fixtures/scenarios/wifi_drop.txt` | **new** |
| `crates/fl-cli/tests/headless_run.rs` | Add `headless_wifi_drop` test |

## 11. Compatibility note for AppState

Changing `Banner.duration` from `Duration` to `Option<Duration>` is a breaking change for the field but `Banner` is constructed only inside `AppState`, so the blast radius is internal. The `Banner` struct is `pub` but no external code constructs it (verified by `git grep "Banner {"`).

## 12. Out of scope

- Automatic switch to USB when USB is replugged mid-session (treat USB only as install lane, not failover backup — different design call).
- Manual reconnect command outside of `fl run` (`fl reconnect` as a separate sub-command).
- Profile/release mode handling (sub-project B).
- Re-using ReconnectManager across multiple devices (sub-project C).
- iOS reconnect (sub-project D — completely different protocol stack).
