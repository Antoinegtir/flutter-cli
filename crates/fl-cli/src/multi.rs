//! Multi-device runtime for `fl run`.
//!
//! Owns N parallel `DeviceSession`s, each backed by its own `FlutterDaemon` +
//! `VmServiceClient` + `ReconnectManager`. Broadcasts keys to every session
//! in parallel.

use anyhow::anyhow;
use fl_adb::{parse_devices_l, pre_pair_wifi, track_devices, CommandRunner, TokioRunner};
use fl_core::{
    AppEvent, BuildMode, DeviceEvent, DeviceSessionState, FlutterEvent, KeyEvent as FlKey,
    LogLevel,
};
use fl_flutter::{resolve_flutter, FlutterDaemon};
use fl_tui::{AppState, TuiRunner};
use fl_vmservice::VmServiceClient;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

#[derive(Clone)]
pub struct DeviceSession {
    pub serial: String,
    pub short_name: String,
    #[allow(dead_code)] // surfaced via session listing logs; reserved for future UI use
    pub display_name: String,
    pub daemon: Arc<Mutex<Option<FlutterDaemon>>>,
    pub vm_client: Arc<Mutex<Option<VmServiceClient>>>,
    pub isolate_id: Arc<Mutex<Option<String>>>,
    /// Captured from the Flutter daemon's first AppStarted event. Needed to
    /// send `app.restart` JSON-RPC back over the daemon's stdin.
    pub app_id: Arc<Mutex<Option<String>>>,
}

impl DeviceSession {
    pub fn new(serial: String, display_name: String) -> Self {
        let short_name = fl_tui::app::short_name_for_serial(&serial);
        Self {
            serial,
            short_name,
            display_name,
            daemon: Arc::new(Mutex::new(None)),
            vm_client: Arc::new(Mutex::new(None)),
            isolate_id: Arc::new(Mutex::new(None)),
            app_id: Arc::new(Mutex::new(None)),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn spawn_session<R: CommandRunner + 'static>(
    runner: Arc<R>,
    flutter: &Path,
    project: &Path,
    serial_to_run: String,
    usb_serial_to_pair: Option<String>,
    no_wifi: bool,
    mode: BuildMode,
    event_tx: mpsc::Sender<AppEvent>,
    vm_mdns_cache: Option<fl_vmservice::mdns::AdCache>,
    prefer_attached: bool,
) -> anyhow::Result<DeviceSession> {
    let display_name = match &usb_serial_to_pair {
        Some(usb) => runner
            .run("adb", &["-s", usb, "shell", "getprop", "ro.product.model"])
            .await
            .ok()
            .map(|o| o.stdout.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| serial_to_run.clone()),
        None => serial_to_run.clone(),
    };
    let session = DeviceSession::new(serial_to_run.clone(), display_name);

    event_tx
        .send(AppEvent::Device(DeviceEvent::SessionState {
            serial: session.serial.clone(),
            state: DeviceSessionState::Connecting,
        }))
        .await
        .ok();

    let final_target = if let (Some(usb), false) = (usb_serial_to_pair.as_deref(), no_wifi) {
        match pre_pair_wifi(runner.as_ref(), usb, 5555).await {
            Ok(t) => {
                event_tx.send(AppEvent::Device(DeviceEvent::WifiPaired {
                    serial: usb.into(),
                    ip: t.ip.clone(),
                    port: t.port,
                })).await.ok();
                t.serial()
            }
            Err(e) => {
                event_tx.send(AppEvent::Device(DeviceEvent::Error(
                    format!("[{}] pre-pair failed: {e}", session.short_name),
                ))).await.ok();
                serial_to_run.clone()
            }
        }
    } else {
        serial_to_run.clone()
    };

    // Reap orphan `iproxy` processes left over from previous `fl run`
    // sessions that exited abruptly (USB unplug → daemon died without
    // cleanup). They accumulate at PPID=1 and eventually wedge usbmuxd:
    // 30+ of them was enough to make subsequent `flutter` spawns return
    // "bash: ...flutter: Operation not permitted". Cheap and idempotent.
    reap_orphan_iproxy(&serial_to_run).await;

    let (flutter_tx, flutter_rx) = mpsc::channel::<FlutterEvent>(64);
    let mode_flag = mode.flutter_flag();
    let mut extra: Vec<&str> = Vec::new();
    if !matches!(mode, BuildMode::Debug) {
        extra.push(mode_flag);
    }
    // Force USB transport when the user picked an attached device. On
    // iOS 17+ Flutter's daemon otherwise tends to pick the slower
    // devicectl-over-Bonjour tunnel ("wireless") even when the cable
    // is plugged in — emitting "Wireless debugging on iOS 26 may be
    // slower than expected" in the log. We surface that connection
    // type from `Device.connection` and translate it here.
    if prefer_attached {
        extra.push("--device-connection");
        extra.push("attached");
    }
    let daemon = FlutterDaemon::spawn(flutter, project, &final_target, &extra, flutter_tx).await?;
    *session.daemon.lock().await = Some(daemon);

    let short_for_logs = session.short_name.clone();
    let serial_for_state = session.serial.clone();
    let event_tx_logs = event_tx.clone();
    let vm_client_slot = session.vm_client.clone();
    let isolate_slot = session.isolate_id.clone();
    let app_id_slot = session.app_id.clone();
    let daemon_slot = session.daemon.clone();
    let _ = vm_mdns_cache; // No longer used — Wi-Fi takeover removed.
    // Captured for the auto-respawn on USB replug.
    let flutter_for_respawn = flutter.to_path_buf();
    let project_for_respawn = project.to_path_buf();
    let mode_for_respawn = mode;
    let prefer_attached_for_respawn = prefer_attached;
    let final_target_for_respawn = final_target.clone();
    tokio::spawn(async move {
        // The daemon may be respawned more than once if the user goes
        // through several unplug↔replug cycles. We loop so all the
        // event-bridging state (vm_connected, etc.) resets cleanly
        // between cycles.
        let mut flutter_rx_local = flutter_rx;
        loop {
            let mut vm_connected = false;
            while let Some(ev) = flutter_rx_local.recv().await {
                let prefixed = match ev {
                    FlutterEvent::Log { level, message } => FlutterEvent::Log {
                        level,
                        message: format!("[{short_for_logs}] {message}"),
                    },
                    FlutterEvent::AppStarted { ref app_id, ref vm_service_uri } => {
                        event_tx_logs.send(AppEvent::Device(DeviceEvent::SessionState {
                            serial: serial_for_state.clone(),
                            state: DeviceSessionState::Ready,
                        })).await.ok();
                        if !app_id.is_empty() {
                            *app_id_slot.lock().await = Some(app_id.clone());
                        }
                        if !vm_connected && !vm_service_uri.is_empty() {
                            vm_connected = true;
                            connect_vm_service(
                                vm_service_uri.clone(),
                                vm_client_slot.clone(),
                                isolate_slot.clone(),
                                event_tx_logs.clone(),
                                short_for_logs.clone(),
                            );
                        }
                        ev
                    }
                    FlutterEvent::Stopped { .. } => {
                        event_tx_logs.send(AppEvent::Device(DeviceEvent::SessionState {
                            serial: serial_for_state.clone(),
                            state: DeviceSessionState::Stopped,
                        })).await.ok();
                        ev
                    }
                    other => other,
                };
                event_tx_logs.send(AppEvent::Flutter(prefixed)).await.ok();
            }

            // Daemon process exited (USB cable yanked, app crashed,
            // user typed `q`, etc.). Wait for the device to come back
            // on USB before respawning. We DO NOT attempt any Wi-Fi
            // fallover — Apple's debugger model means the app on the
            // phone is gone the moment lldb loses its tunnel anyway,
            // and the previous Wi-Fi takeover implementation only
            // pretended to keep things going while in fact destabilizing
            // the iPhone. Cleaner UX: wait for the cable, respawn the
            // whole flutter run, keep the TUI alive throughout.
            *daemon_slot.lock().await = None;
            *vm_client_slot.lock().await = None;
            *isolate_slot.lock().await = None;
            event_tx_logs.send(AppEvent::Device(DeviceEvent::SessionState {
                serial: serial_for_state.clone(),
                state: DeviceSessionState::Stopped,
            })).await.ok();
            event_tx_logs.send(AppEvent::Flutter(FlutterEvent::Log {
                level: LogLevel::Warn,
                message: format!(
                    "[{short_for_logs}] USB session ended — replug the cable to resume \
                     (keeping TUI alive; logs scrollable with ↑/↓)"
                ),
            })).await.ok();

            // Wait for the same UDID to reappear over USB. We poll
            // devicectl rather than subscribing to the device event
            // stream because that stream is consumed by AppState and
            // we'd need a tee. Polling every 2 s is plenty.
            let xcrun = fl_ios::Xcrun::new(fl_adb::TokioRunner);
            let mut last_state_logged: Option<bool> = None;
            loop {
                let devs = fl_ios::list_apple_devices(&xcrun).await;
                let back_on_usb = devs.iter().any(|d| {
                    d.serial == serial_for_state
                        && matches!(d.connection, fl_core::ConnectionKind::Usb)
                        && matches!(d.state, fl_core::DeviceState::Online)
                });
                if back_on_usb {
                    event_tx_logs.send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Info,
                        message: format!(
                            "[{short_for_logs}] 🔌 USB reconnected — relaunching app on device"
                        ),
                    })).await.ok();
                    break;
                }
                if last_state_logged != Some(false) {
                    last_state_logged = Some(false);
                    event_tx_logs.send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Debug,
                        message: format!("[{short_for_logs}] waiting for USB cable…"),
                    })).await.ok();
                }
                // 3 s is the sweet spot between "feels responsive when
                // the user replugs" and "doesn't burn CPU re-parsing
                // the heavy devicectl JSON every 2 s during idle".
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }

            // Cable is back — spawn a fresh `flutter run`. We reuse
            // the original args (mode + prefer_attached) so the new
            // session is launched the same way the user originally
            // asked for.
            let (new_tx, new_rx) = mpsc::channel::<FlutterEvent>(64);
            flutter_rx_local = new_rx;
            let mut respawn_extra: Vec<&str> = Vec::new();
            let mode_flag = mode_for_respawn.flutter_flag();
            if !matches!(mode_for_respawn, BuildMode::Debug) {
                respawn_extra.push(mode_flag);
            }
            if prefer_attached_for_respawn {
                respawn_extra.push("--device-connection");
                respawn_extra.push("attached");
            }
            // Reap the iproxy zombies from the just-dead daemon before
            // spawning the new one — otherwise EPERM kicks in after a
            // few cycles.
            reap_orphan_iproxy(&serial_for_state).await;
            match FlutterDaemon::spawn(
                &flutter_for_respawn,
                &project_for_respawn,
                &final_target_for_respawn,
                &respawn_extra,
                new_tx,
            )
            .await
            {
                Ok(d) => {
                    *daemon_slot.lock().await = Some(d);
                    event_tx_logs.send(AppEvent::Device(DeviceEvent::SessionState {
                        serial: serial_for_state.clone(),
                        state: DeviceSessionState::Connecting,
                    })).await.ok();
                }
                Err(e) => {
                    event_tx_logs.send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Error,
                        message: format!("[{short_for_logs}] respawn failed: {e} — session ended"),
                    })).await.ok();
                    return;
                }
            }
        }
    });

    Ok(session)
}


/// Kill orphan `iproxy` (libusbmuxd port forwarder) processes left over
/// from previous Flutter daemons that died abruptly. Each `flutter run`
/// over USB spawns 2+ iproxy children; when the daemon process is
/// killed without cleanup hooks running (USB unplug crashes the chain),
/// these become PPID=1 zombies. After enough sessions, usbmuxd starts
/// refusing new spawns with EPERM, which surfaces as `bash:
/// .../flutter: Operation not permitted` on the next `flutter` invoke.
///
/// We do this asynchronously and ignore errors — `pkill` returning
/// exit 1 just means there was nothing to kill, which is fine.
async fn reap_orphan_iproxy(udid: &str) {
    // Be specific so we don't kill someone else's adb/iproxy session.
    let pattern = format!("iproxy.*--udid {udid}");
    let _ = tokio::process::Command::new("pkill")
        .args(["-9", "-f", &pattern])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
}

/// Spawn a task that connects to the Flutter VM Service at `uri`, stores the
/// resulting client + first isolate ID in the session's slots, and bridges
/// VM Service events into the global AppEvent channel.
fn connect_vm_service(
    uri: String,
    client_slot: Arc<Mutex<Option<VmServiceClient>>>,
    isolate_slot: Arc<Mutex<Option<String>>>,
    event_tx: mpsc::Sender<AppEvent>,
    short_name: String,
) {
    tokio::spawn(async move {
        let (vm_tx, mut vm_rx) = mpsc::channel::<fl_core::VmEvent>(128);
        // Bridge VmEvents → AppEvent::Vm.
        let event_tx_bridge = event_tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = vm_rx.recv().await {
                event_tx_bridge.send(AppEvent::Vm(ev)).await.ok();
            }
        });

        let client = match VmServiceClient::connect(&uri, vm_tx).await {
            Ok(c) => c,
            Err(e) => {
                event_tx.send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Warn,
                    message: format!("[{short_name}] VM Service connect failed: {e}"),
                })).await.ok();
                return;
            }
        };

        // Subscribe to streams.
        let _ = client.stream_listen("Stdout").await;
        let _ = client.stream_listen("Stderr").await;
        let _ = client.stream_listen("Isolate").await;
        let _ = client.stream_listen("Extension").await;

        // Isolate isn't always immediately available — retry briefly.
        let mut isolate_id: Option<String> = None;
        for _ in 0..40 {
            if let Ok(id) = client.get_first_isolate_id().await {
                isolate_id = Some(id);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }

        if let Some(id) = isolate_id {
            *isolate_slot.lock().await = Some(id.clone());
            *client_slot.lock().await = Some(client.clone());
            event_tx.send(AppEvent::Flutter(FlutterEvent::Log {
                level: LogLevel::Info,
                message: format!("[{short_name}] VM Service ready"),
            })).await.ok();
            // Memory usage is pull-only on the VM Service — poll once a
            // second and synthesize `VmEvent::GcStats` so the Performance
            // panel's memory line actually moves. We refresh the isolate
            // id on every tick because hot-restart issues a brand new
            // isolate and the old one starts returning "Sentinel /
            // Collected", which is what was previously freezing Memory at
            // 0 MB.
            let mem_client = client.clone();
            let mem_tx = event_tx.clone();
            tokio::spawn(async move {
                let mut ticker =
                    tokio::time::interval(std::time::Duration::from_secs(1));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut consecutive_failures: u32 = 0;
                loop {
                    ticker.tick().await;
                    let iso = match mem_client.get_first_isolate_id().await {
                        Ok(id) => id,
                        Err(_) => {
                            consecutive_failures += 1;
                            if consecutive_failures > 60 {
                                break; // VM unreachable for a minute → give up.
                            }
                            continue;
                        }
                    };
                    match mem_client.get_memory_usage_mb(&iso).await {
                        Ok((used, total)) => {
                            consecutive_failures = 0;
                            if mem_tx
                                .send(AppEvent::Vm(fl_core::VmEvent::GcStats {
                                    used_mb: used,
                                    total_mb: total,
                                }))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(_) => {
                            consecutive_failures += 1;
                            if consecutive_failures > 60 {
                                break;
                            }
                        }
                    }
                }
            });
        } else {
            event_tx.send(AppEvent::Flutter(FlutterEvent::Log {
                level: LogLevel::Warn,
                message: format!("[{short_name}] VM Service: no isolate found after 10s"),
            })).await.ok();
        }
    });
}

pub async fn broadcast_key(
    key: FlKey,
    sessions: &[DeviceSession],
    events: &mpsc::Sender<AppEvent>,
    brightness_value: Option<bool>,
    paint_on: bool,
    platform_is_ios: bool,
    perf_overlay_on: bool,
) {
    // Hot reload (`r`) and hot restart (`R`) normally go through the
    // Flutter daemon's stdin via `app.restart`. After a USB unplug the
    // daemon is dead — but the VM Service over the coredevice tunnel
    // is still alive. We can fall back to `reloadSources` (VM Service
    // RPC) for `r`. Hot restart has no VM Service equivalent, so it's
    // simply unavailable post-unplug.
    if matches!(key, FlKey::Char('r') | FlKey::Char('R')) {
        let full = matches!(key, FlKey::Char('R'));
        for s in sessions {
            let short = s.short_name.clone();
            let app_id_opt = s.app_id.lock().await.clone();
            // Try the daemon path first when it's alive.
            let daemon_alive = s.daemon.lock().await.is_some();
            let daemon_succeeded = if daemon_alive {
                if let Some(app_id) = app_id_opt.clone() {
                    let mut daemon_guard = s.daemon.lock().await;
                    if let Some(d) = daemon_guard.as_mut() {
                        match d.send_app_restart(&app_id, full).await {
                            Ok(()) => {
                                let kind = if full { "restart" } else { "reload" };
                                events.send(AppEvent::Flutter(FlutterEvent::Log {
                                    level: LogLevel::Info,
                                    message: format!("[{short}] {kind} requested"),
                                })).await.ok();
                                true
                            }
                            Err(_) => false, // Pipe broken → daemon died mid-flight.
                        }
                    } else { false }
                } else { false }
            } else { false };

            if daemon_succeeded {
                continue;
            }

            // Daemon-less path: VM Service for hot reload, error for restart.
            if full {
                events.send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Warn,
                    message: format!(
                        "[{short}] hot restart unavailable after USB unplug — \
                         use `r` for hot reload or relaunch with `fl run`"
                    ),
                })).await.ok();
                continue;
            }

            let vm = s.vm_client.lock().await.clone();
            let iso = s.isolate_id.lock().await.clone();
            let (Some(client), Some(iso)) = (vm, iso) else {
                events.send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Warn,
                    message: format!("[{short}] no VM Service yet, can't reload"),
                })).await.ok();
                continue;
            };
            match client.hot_reload(&iso).await {
                Ok(_) => {
                    events.send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Info,
                        message: format!("[{short}] reload requested (via VM Service over tunnel)"),
                    })).await.ok();
                }
                Err(e) => {
                    events.send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Error,
                        message: format!("[{short}] reload failed: {e:#}"),
                    })).await.ok();
                }
            }
        }
        return;
    }

    // Brightness / paint / platform / perf are VM Service extensions.
    let mut futures = Vec::new();
    for s in sessions {
        let Some(client) = s.vm_client.lock().await.clone() else {
            // Silently ignore extension keys that arrive before the VM
            // Service is up. The footer hides r/R/b/p/o/P during
            // build/install, so the user can't be tempted to press
            // them — but they may still arrive via paste / typeahead /
            // muscle memory. Emitting a warn here would just spam the
            // log; better to drop them silently.
            continue;
        };
        // Refresh the isolate id on every press. The cached value gets
        // stale across hot restarts (Flutter mints a fresh isolate) —
        // calling an extension against the old id returns a "Sentinel /
        // Collected" sentinel object, which we'd misreport as success.
        // Worst case after USB unplug, hitting a dead isolate could
        // even kill the running app.
        let iso = match client.get_first_isolate_id().await {
            Ok(id) => {
                *s.isolate_id.lock().await = Some(id.clone());
                id
            }
            Err(_) => {
                if let Some(cached) = s.isolate_id.lock().await.clone() {
                    cached
                } else {
                    events.send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Warn,
                        message: format!("[{}] no live isolate — ignoring {:?}", s.short_name, key),
                    })).await.ok();
                    continue;
                }
            }
        };
        let short = s.short_name.clone();
        let key_copy = key;
        futures.push(async move {
            let res = match key_copy {
                FlKey::Char('b') => client.set_brightness(&iso, brightness_value).await,
                FlKey::Char('p') => client.toggle_debug_paint(&iso, paint_on).await,
                FlKey::Char('o') => client.toggle_platform(&iso, platform_is_ios).await,
                FlKey::Char('P') => client.toggle_performance_overlay(&iso, perf_overlay_on).await,
                _ => return None,
            };
            Some((short, res))
        });
    }
    let results = futures_util::future::join_all(futures).await;
    for outcome in results.into_iter().flatten() {
        let (short, res) = outcome;
        match res {
            Ok(value) => {
                // The VM Service returns `{"type":"Sentinel", ...}` instead
                // of an error when the target isolate has been collected /
                // replaced (e.g. after a hot restart). That's effectively a
                // failure: nothing on the device acted on the call. Treat
                // it as such instead of celebrating with an "OK" line.
                let is_sentinel = value
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    == Some("Sentinel");
                if is_sentinel {
                    let label = match key {
                        FlKey::Char('b') => "Theme",
                        FlKey::Char('p') => "Debug paint",
                        FlKey::Char('o') => "Platform override",
                        FlKey::Char('P') => "Performance overlay",
                        _ => "Extension",
                    };
                    events.send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Warn,
                        message: format!(
                            "[{short}] {label}: isolate was collected (likely after hot \
                             restart) — try the key again to re-target the new isolate"
                        ),
                    })).await.ok();
                } else {
                    // Translate each extension's structured response into a
                    // single human-readable status line. The raw JSON
                    // (`{"method":"...","type":"_extensionType","value":"..."}`)
                    // is useless to the user — show what actually changed
                    // on the device instead.
                    let pretty = pretty_status(key, &value);
                    events.send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Info,
                        message: format!("[{short}] {pretty}"),
                    })).await.ok();
                }
            }
            Err(e) => {
                events.send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Error,
                    message: format!("[{short}] {key:?} -> {e}"),
                })).await.ok();
            }
        }
    }
}

/// Render the VM Service response to one of our extension keys as a
/// short human-readable status line — no raw JSON in the log.
fn pretty_status(key: FlKey, value: &serde_json::Value) -> String {
    match key {
        FlKey::Char('b') => {
            let raw = value.get("value").and_then(serde_json::Value::as_str).unwrap_or("");
            let label = match raw {
                "Brightness.light" => "☀️ Theme → Light",
                "Brightness.dark" => "🌙 Theme → Dark",
                "default" => "⚙️ Theme → Settings (follows device iOS/Android Settings preference)",
                other => return format!("Theme → {other}"),
            };
            label.to_string()
        }
        FlKey::Char('p') => {
            // Flutter returns "enabled" as a string "true"/"false".
            let on = matches!(
                value.get("enabled").and_then(serde_json::Value::as_str),
                Some("true")
            );
            if on {
                "🟦 Debug paint ON (widget bounds visible on device)".into()
            } else {
                "🟦 Debug paint OFF".into()
            }
        }
        FlKey::Char('P') => {
            let on = matches!(
                value.get("enabled").and_then(serde_json::Value::as_str),
                Some("true")
            );
            if on {
                "📊 Perf overlay ON (FPS bars on device)".into()
            } else {
                "📊 Perf overlay OFF".into()
            }
        }
        FlKey::Char('o') => {
            let raw = value.get("value").and_then(serde_json::Value::as_str).unwrap_or("");
            match raw {
                "iOS" => "🍎 Platform override → iOS (Cupertino widgets)".into(),
                "android" => "🤖 Platform override → Android (Material widgets)".into(),
                "fuchsia" => "🟣 Platform override → Fuchsia".into(),
                other => format!("Platform override → {other}"),
            }
        }
        _ => format!("Extension OK → {}", compact_json(value)),
    }
}

/// One-line summary of a JSON value for log output.
fn compact_json(v: &serde_json::Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 120 {
        s.chars().take(117).chain("…".chars()).collect()
    } else {
        s
    }
}

pub fn resolve_flutter_path() -> anyhow::Result<PathBuf> {
    resolve_flutter(None, std::env::var("FLUTTER_ROOT").ok().as_deref(), dirs_home())
        .ok_or_else(|| anyhow!("flutter binary not found"))
}

fn dirs_home() -> Option<&'static Path> {
    static HOME: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()))
        .as_deref()
}

pub async fn run_multi(
    project: Option<PathBuf>,
    devices_arg: Vec<String>,
    all: bool,
    no_picker: bool,
    no_wifi: bool,
    no_tui: bool,
    mode: BuildMode,
) -> anyhow::Result<()> {
    let project = project.unwrap_or_else(|| std::env::current_dir().unwrap());
    let flutter = resolve_flutter_path()?;
    let runner = Arc::new(TokioRunner);

    // Startup sweep: kill any leftover `iproxy` zombies from a
    // previous `fl run` that exited badly. Without this, after a few
    // bad sessions, macOS's usbmuxd starts refusing new iproxy spawns
    // and the next `flutter run` fails with EPERM. We don't know the
    // UDID yet here, so we use a broader pattern (any libusbmuxd
    // iproxy in Flutter's cache dir) — safe because nothing else
    // uses that specific binary path.
    let _ = tokio::process::Command::new("pkill")
        .args(["-9", "-f", "flutter/bin/cache/artifacts/libusbmuxd/iproxy"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;

    let listed = runner.run("adb", &["devices", "-l"]).await?;
    let mut all_devices = parse_devices_l(&listed.stdout);
    let xcrun = fl_ios::Xcrun::new(TokioRunner);
    all_devices.extend(fl_ios::list_apple_devices(&xcrun).await);

    let plain = no_tui || std::env::var_os("FL_HEADLESS").is_some();
    let headless_event_dump = std::env::var_os("FL_HEADLESS").is_some(); // tests want raw {ev:?}
    let chosen: Vec<String> = if !devices_arg.is_empty() {
        devices_arg
    } else if all {
        if all_devices.is_empty() {
            return Err(anyhow!("--all specified but no devices attached"));
        }
        all_devices.iter().map(|d| d.serial.clone()).collect()
    } else if all_devices.len() <= 1 || no_picker || plain {
        all_devices.first().map(|d| vec![d.serial.clone()]).unwrap_or_default()
    } else {
        run_picker(&all_devices).await?
    };

    if chosen.is_empty() {
        return Err(anyhow!("no devices to run on"));
    }

    let (event_tx, mut event_rx) = mpsc::channel::<AppEvent>(256);

    // Boot the Dart-VM-Service mDNS browser once. The cache it populates
    // is shared with every session so an iOS USB unplug can fall over to
    // a direct Wi-Fi WebSocket connection without dropping the dev loop.
    // Browser failure (e.g. no IPv4 multicast) is non-fatal — sessions
    // just lose the takeover capability.
    let vm_mdns_cache = match fl_vmservice::mdns::spawn_browser() {
        Ok((cache, _handle)) => Some(cache),
        Err(e) => {
            event_tx.send(AppEvent::Flutter(FlutterEvent::Log {
                level: LogLevel::Warn,
                message: format!("mDNS browser unavailable: {e} — Wi-Fi takeover disabled"),
            })).await.ok();
            None
        }
    };

    let mut sessions: Vec<DeviceSession> = Vec::new();
    for serial in &chosen {
        let usb_pair = all_devices
            .iter()
            .find(|d| d.serial == *serial
                  && matches!(d.connection, fl_core::ConnectionKind::Usb)
                  && (d.platform.as_deref() == Some("android") || d.platform.is_none()))
            .map(|d| d.serial.clone());
        // If our discovery says the chosen device is wired (devicectl /
        // adb reported USB transport), force Flutter to use the attached
        // tunnel — otherwise iOS 17+ silently routes through the slower
        // Bonjour transport even when the cable is plugged in.
        let prefer_attached = all_devices
            .iter()
            .find(|d| d.serial == *serial)
            .map(|d| matches!(d.connection, fl_core::ConnectionKind::Usb))
            .unwrap_or(false);
        let s = spawn_session(
            runner.clone(),
            &flutter,
            &project,
            serial.clone(),
            usb_pair,
            no_wifi,
            mode,
            event_tx.clone(),
            vm_mdns_cache.clone(),
            prefer_attached,
        )
        .await?;
        sessions.push(s);
    }

    {
        let tx = event_tx.clone();
        tokio::spawn(async move {
            let (dev_tx, mut dev_rx) = mpsc::channel(32);
            tokio::spawn(async move {
                if let Err(e) = track_devices(dev_tx).await {
                    tracing::warn!("track-devices loop ended: {e}");
                }
            });
            while let Some(ev) = dev_rx.recv().await {
                tx.send(AppEvent::Device(ev)).await.ok();
            }
        });
    }

    {
        let tx = event_tx.clone();
        tokio::spawn(async move {
            let (dev_tx, mut dev_rx) = mpsc::channel(32);
            tokio::spawn(async move {
                let xcrun = fl_ios::Xcrun::new(TokioRunner);
                fl_ios::watch_apple_devices(xcrun, dev_tx).await;
            });
            while let Some(ev) = dev_rx.recv().await {
                tx.send(AppEvent::Device(ev)).await.ok();
            }
        });
    }

    if plain {
        let app_name = project
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("app")
            .to_string();
        let mut state = AppState::new(app_name, "debug".into());
        eprintln!(
            "fl run --no-tui · {} session{} · Ctrl-C to quit",
            sessions.len(),
            if sessions.len() == 1 { "" } else { "s" }
        );
        let started = std::time::Instant::now();
        // Race events vs ctrl-c so the user can always abort.
        loop {
            tokio::select! {
                biased;
                _ = tokio::signal::ctrl_c() => {
                    eprintln!("\nReceived Ctrl-C, shutting down…");
                    break;
                }
                ev = event_rx.recv() => {
                    let Some(ev) = ev else { break };
                    if headless_event_dump {
                        println!("{ev:?}");
                    } else {
                        print_event_pretty(&ev, started.elapsed());
                    }
                    state.apply(ev);
                    if !state.active_sessions.is_empty()
                        && state.active_sessions.iter().all(|s| {
                            matches!(s.state,
                                fl_core::DeviceSessionState::Stopped
                              | fl_core::DeviceSessionState::Failed)
                        })
                    {
                        break;
                    }
                }
            }
        }
        shutdown_sessions_fast(&sessions).await;
        return Ok(());
    }

    let app_name = project.file_name().and_then(|n| n.to_str()).unwrap_or("app").to_string();
    let mut state = AppState::new(app_name, "debug".into());
    // Use an INLINE viewport (Claude-Code style): the TUI box sits at
    // the bottom of the terminal, the user's command history (and
    // streaming flutter logs) flow through the scrollback above it.
    // 16 rows fit: status header (3) + perf/devices (10) + footer
    // (1) + margin. Logs are NOT inside the viewport — they're
    // printed above it via `TuiRunner::print_above_viewport` so they
    // scroll naturally with the rest of the terminal.
    let mut tui = TuiRunner::init_inline_with_banner(16)?;
    let (keys_tx, mut keys_rx) = mpsc::channel::<FlKey>(16);

    // Key dispatcher: each keystroke from the TUI is broadcast to every
    // session's daemon (r/R) or VM Service (b/p/o/P).
    {
        let sessions_for_keys: Vec<DeviceSession> = sessions.to_vec();
        let event_tx_keys = event_tx.clone();
        let brightness = state.brightness_handle();
        let paint = state.debug_paint_handle();
        let platform = state.platform_handle();
        let perf_overlay = state.perf_overlay_handle();
        tokio::spawn(async move {
            while let Some(k) = keys_rx.recv().await {
                if matches!(
                    k,
                    FlKey::Char('r') | FlKey::Char('R') | FlKey::Char('b')
                  | FlKey::Char('p') | FlKey::Char('o') | FlKey::Char('P')
                ) {
                    let bs = brightness.load(std::sync::atomic::Ordering::Relaxed);
                    let brightness_value: Option<bool> = match bs {
                        fl_tui::app::BRIGHTNESS_LIGHT => Some(false),
                        fl_tui::app::BRIGHTNESS_DARK  => Some(true),
                        _ => None, // BRIGHTNESS_SYSTEM
                    };
                    let paint_on = paint.load(std::sync::atomic::Ordering::Relaxed);
                    let platform_is_ios = platform.load(std::sync::atomic::Ordering::Relaxed);
                    let perf_overlay_on = perf_overlay.load(std::sync::atomic::Ordering::Relaxed);
                    broadcast_key(
                        k,
                        &sessions_for_keys,
                        &event_tx_keys,
                        brightness_value,
                        paint_on,
                        platform_is_ios,
                        perf_overlay_on,
                    )
                    .await;
                }
            }
        });
    }

    let result = tui.run(&mut state, &mut event_rx, keys_tx).await;

    // Restore the terminal IMMEDIATELY so the user gets their shell back.
    // Daemon shutdown then runs against a normal terminal, with shorter
    // timeouts and parallel execution. We deliberately do NOT dump a
    // session / log summary here — pressing `q` should leave the user
    // with a clean shell prompt, the same way Claude Code does on
    // exit. The log lines they care about already live in the
    // terminal's scrollback (printed there inline by
    // `TuiRunner::print_above_viewport`).
    let _ = tui.restore();
    shutdown_sessions_fast(&sessions).await;
    result
}

/// Format an AppEvent as a single colored line on stdout for --no-tui mode.
/// Flushes stdout after writing so output is live even when piped through
/// `tee` or redirected to a file.
fn print_event_pretty(ev: &AppEvent, elapsed: std::time::Duration) {
    use std::io::Write;
    let ts = format!("{:>4}.{:01}s", elapsed.as_secs(), elapsed.subsec_millis() / 100);
    let line = match ev {
        AppEvent::Flutter(FlutterEvent::Log { level, message }) => {
            let tag = match level {
                LogLevel::Error => "\x1b[1;31mERROR\x1b[0m",
                LogLevel::Warn  => "\x1b[1;33mWARN \x1b[0m",
                LogLevel::Info  => "\x1b[1;36mINFO \x1b[0m",
                LogLevel::Debug => "\x1b[90mDEBUG\x1b[0m",
                LogLevel::Trace => "\x1b[90mTRACE\x1b[0m",
            };
            format!("\x1b[90m{ts}\x1b[0m {tag} {message}")
        }
        AppEvent::Flutter(FlutterEvent::AppStarted { app_id, vm_service_uri }) => {
            format!("\x1b[90m{ts}\x1b[0m \x1b[1;32mSTART\x1b[0m app={app_id} vm={vm_service_uri}")
        }
        AppEvent::Flutter(FlutterEvent::Stopped { exit_code }) => {
            format!("\x1b[90m{ts}\x1b[0m \x1b[1;31mSTOP \x1b[0m exit_code={exit_code:?}")
        }
        AppEvent::Flutter(FlutterEvent::Progress { id, message, finished }) => {
            let mark = if *finished { "✓" } else { "…" };
            format!("\x1b[90m{ts}\x1b[0m \x1b[1;34mPROG \x1b[0m {mark} [{id}] {message}")
        }
        AppEvent::Flutter(other) => {
            format!("\x1b[90m{ts}\x1b[0m \x1b[1;36mFLU  \x1b[0m {other:?}")
        }
        AppEvent::Device(d) => {
            format!("\x1b[90m{ts}\x1b[0m \x1b[1;35mDEV  \x1b[0m {d:?}")
        }
        AppEvent::Vm(v) => {
            format!("\x1b[90m{ts}\x1b[0m \x1b[1;34mVM   \x1b[0m {v:?}")
        }
        AppEvent::Key(_) | AppEvent::Tick => return,
    };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Send `q` to every daemon in parallel, wait up to 1 s for each to exit,
/// then force-kill any stragglers. Total wallclock ~1.5 s max regardless of N.
async fn shutdown_sessions_fast(sessions: &[DeviceSession]) {
    let quits = sessions.iter().map(|s| {
        let daemon = s.daemon.clone();
        async move {
            let mut guard = daemon.lock().await;
            if let Some(d) = guard.as_mut() {
                let _ = tokio::time::timeout(std::time::Duration::from_millis(500), d.send_quit()).await;
            }
        }
    });
    futures_util::future::join_all(quits).await;

    let waits = sessions.iter().map(|s| {
        let daemon = s.daemon.clone();
        async move {
            let mut guard = daemon.lock().await;
            if let Some(d) = guard.as_mut() {
                if tokio::time::timeout(std::time::Duration::from_secs(1), d.wait()).await.is_err() {
                    let _ = d.kill().await;
                    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), d.wait()).await;
                }
            }
        }
    });
    futures_util::future::join_all(waits).await;
}

async fn run_picker(devices: &[fl_core::Device]) -> anyhow::Result<Vec<String>> {
    use fl_tui::{DevicePickerInput, DevicePickerOutcome, DevicePickerView};
    let mut view = DevicePickerView::with_devices(devices.to_vec());
    let (_tx, mut rx) = mpsc::channel::<DevicePickerInput>(1);
    // Inline viewport so the device picker doesn't blow away the
    // user's terminal history. ~14 rows accommodates a header + a
    // handful of devices + footer; the picker view clamps its own
    // list to the available rows so longer device lists still scroll
    // inside the box rather than overflowing it.
    let mut tui = TuiRunner::init_inline(14)?;
    let r = tui.run_view(&mut view, &mut rx).await;
    let _ = tui.restore();
    r?;
    match view.outcome {
        Some(DevicePickerOutcome::Picked(serials)) => Ok(serials),
        Some(DevicePickerOutcome::Cancelled) | None => Err(anyhow!("device selection cancelled")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_session_new_derives_short_name() {
        let s = DeviceSession::new("Pixel_8_ABCDEFG".into(), "Pixel 8".into());
        assert_eq!(s.serial, "Pixel_8_ABCDEFG");
        assert_eq!(s.short_name, "Pixel8AB"); // 8 alphanumeric chars
        assert_eq!(s.display_name, "Pixel 8");
    }
}
