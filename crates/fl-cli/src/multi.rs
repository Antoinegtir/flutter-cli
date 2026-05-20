//! Multi-device runtime for `fl run`.
//!
//! Owns N parallel `DeviceSession`s, each backed by its own `FlutterDaemon` +
//! `VmServiceClient` + `ReconnectManager`. Broadcasts keys to every session
//! in parallel.

use anyhow::anyhow;
use fl_adb::{parse_devices_l, pre_pair_wifi, track_devices, CommandRunner, TokioRunner};
use fl_core::{
    AppEvent, BuildMode, DeviceEvent, DeviceSessionState, FlutterEvent, KeyEvent as FlKey, LogLevel,
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
    /// DevTools URL the Flutter daemon emits as `app.devTools: …` once
    /// the VM Service is up. Captured live from log lines and used by
    /// the `d` keybind to spawn `open <url>` (or `xdg-open` on Linux)
    /// in the user's default browser.
    pub devtools_uri: Arc<Mutex<Option<String>>>,
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
            devtools_uri: Arc::new(Mutex::new(None)),
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
    extra_user_args: Vec<String>,
    // True when more than one device is running in parallel — log
    // lines then get a `[Device Name] ` prefix so the user can tell
    // streams apart. Single-device sessions skip the prefix; it's
    // just noise when there's nothing to disambiguate.
    multi_device: bool,
    // Pre-resolved human-readable name from the caller's device
    // discovery (e.g. "iPhone Antoine", "sdk gphone64 arm64"). Used
    // as-is when non-empty; we only fall back to the adb getprop
    // probe / serial when the caller didn't have one.
    discovered_name: Option<String>,
) -> anyhow::Result<DeviceSession> {
    let display_name = match (discovered_name.as_deref(), &usb_serial_to_pair) {
        // Caller knew the device name already — use it directly.
        (Some(n), _) if !n.is_empty() && n != serial_to_run => n.to_string(),
        // Android USB path: ask the device for its marketing model.
        (_, Some(usb)) => runner
            .run("adb", &["-s", usb, "shell", "getprop", "ro.product.model"])
            .await
            .ok()
            .map(|o| o.stdout.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| serial_to_run.clone()),
        // Last-resort fallback: the raw serial.
        _ => serial_to_run.clone(),
    };
    let session = DeviceSession::new(serial_to_run.clone(), display_name);

    event_tx
        .send(AppEvent::Device(DeviceEvent::SessionState {
            serial: session.serial.clone(),
            state: DeviceSessionState::Connecting,
        }))
        .await
        .ok();

    // Push the resolved display name into AppState immediately via a
    // synthetic Discovered event. Otherwise the Performance / Devices
    // panels show the raw serial until `track_devices` happens to fire
    // another Discovered (which itself carries `name == serial` since
    // the `host:track-devices` payload has no model field). Doing this
    // here means panels render "SM A145R" instead of "R8YW90VSSGD"
    // from the first frame the session is alive.
    if !session.display_name.is_empty() && session.display_name != session.serial {
        let platform = if usb_serial_to_pair.is_some() {
            Some("android".to_string())
        } else {
            None
        };
        event_tx
            .send(AppEvent::Device(DeviceEvent::Discovered(fl_core::Device {
                serial: session.serial.clone(),
                name: session.display_name.clone(),
                model: Some(session.display_name.clone()),
                connection: fl_core::ConnectionKind::Usb,
                state: fl_core::DeviceState::Online,
                ip: None,
                android_version: None,
                battery: None,
                platform,
            })))
            .await
            .ok();
    }

    let final_target = if let (Some(usb), false) = (usb_serial_to_pair.as_deref(), no_wifi) {
        match pre_pair_wifi(runner.as_ref(), usb, 5555).await {
            Ok(t) => {
                event_tx
                    .send(AppEvent::Device(DeviceEvent::WifiPaired {
                        serial: usb.into(),
                        ip: t.ip.clone(),
                        port: t.port,
                    }))
                    .await
                    .ok();
                t.serial()
            }
            Err(e) => {
                let pfx = if multi_device {
                    format!("[{}] ", session.display_name)
                } else {
                    String::new()
                };
                event_tx
                    .send(AppEvent::Device(DeviceEvent::Error(format!(
                        "{pfx}pre-pair failed: {e}"
                    ))))
                    .await
                    .ok();
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
    // User-supplied pass-through args (after `--` on the `fl run`
    // command line). Appended LAST so explicit user choices win over
    // any defaults we added above.
    for a in &extra_user_args {
        extra.push(a.as_str());
    }
    let daemon = FlutterDaemon::spawn(flutter, project, &final_target, &extra, flutter_tx).await?;
    *session.daemon.lock().await = Some(daemon);

    // Use the user-recognisable display name (e.g. "iPhone Antoine",
    // "SM A145R") and prefix log lines with it ONLY when more than
    // one device is live. Solo runs get unprefixed logs — cleaner
    // signal-to-noise when there's no ambiguity.
    let short_for_logs = if multi_device {
        format!("[{}] ", session.display_name)
    } else {
        String::new()
    };
    let serial_for_state = session.serial.clone();
    let display_name_for_logs = session.display_name.clone();
    let event_tx_logs = event_tx.clone();
    let vm_client_slot = session.vm_client.clone();
    let isolate_slot = session.isolate_id.clone();
    let app_id_slot = session.app_id.clone();
    let devtools_slot = session.devtools_uri.clone();
    let daemon_slot = session.daemon.clone();
    let _ = vm_mdns_cache; // No longer used — Wi-Fi takeover removed.
                           // Captured for the auto-respawn on USB replug.
    let flutter_for_respawn = flutter.to_path_buf();
    let project_for_respawn = project.to_path_buf();
    let mode_for_respawn = mode;
    let prefer_attached_for_respawn = prefer_attached;
    let final_target_for_respawn = final_target.clone();
    let extra_user_args_for_respawn = extra_user_args.clone();
    tokio::spawn(async move {
        // The daemon may be respawned more than once if the user goes
        // through several unplug↔replug cycles. We loop so all the
        // event-bridging state (vm_connected, etc.) resets cleanly
        // between cycles.
        let mut flutter_rx_local = flutter_rx;
        // Did *any* respawn cycle ever reach `AppStarted`? Used to
        // decide whether a daemon death is "session lost, recover"
        // (true) vs. "run never started — config / device error, bail
        // fast" (false). Without this guard, `flutter run -d
        // emulator-5554` (no such device) drops into the iOS USB
        // replug poll and waits forever.
        let mut ever_started = false;
        // Track whether the "✓ compiled and launched" log has been
        // emitted yet for this session. The Flutter daemon can fire
        // `AppStarted` more than once over a session's lifetime
        // (after a hot restart, after a VM Service re-attach, …),
        // and we don't want a green ✓ line showing up every time
        // — the build was already done.
        let mut compiled_emitted = false;
        // Timestamps of consecutive daemon deaths. If 3+ happen
        // within 30s, we give up the respawn loop with a helpful
        // message — typically means the user's Flutter setup is
        // broken (NDK mismatch, compile error, …) and looping just
        // wastes their cycles.
        let mut death_history: Vec<std::time::Instant> = Vec::new();
        // Buffer the last few log lines so we can echo them back in
        // the "session never started" failure message — the actual
        // reason from Flutter is often the only thing the user
        // needs to see.
        let mut last_logs: std::collections::VecDeque<String> =
            std::collections::VecDeque::with_capacity(6);
        loop {
            let mut vm_connected = false;
            while let Some(ev) = flutter_rx_local.recv().await {
                let prefixed = match ev {
                    FlutterEvent::Log { level, message } => {
                        if let Some(rest) = message.strip_prefix("app.devTools: ") {
                            *devtools_slot.lock().await = Some(rest.trim().to_string());
                        }
                        last_logs.push_back(message.clone());
                        if last_logs.len() > 6 {
                            last_logs.pop_front();
                        }
                        FlutterEvent::Log {
                            level,
                            message: format!("{short_for_logs}{message}"),
                        }
                    }
                    FlutterEvent::AppStarted {
                        ref app_id,
                        ref vm_service_uri,
                    } => {
                        ever_started = true;
                        event_tx_logs
                            .send(AppEvent::Device(DeviceEvent::SessionState {
                                serial: serial_for_state.clone(),
                                state: DeviceSessionState::Ready,
                            }))
                            .await
                            .ok();
                        // Emit a green "✓ compiled" line ONCE per
                        // session — `log_style_for` in fl-tui sees
                        // the `✓` and upgrades the line to
                        // `theme.success`. Subsequent `AppStarted`
                        // events (hot restart, VM re-attach) are
                        // ignored to avoid the doubled green line.
                        if !compiled_emitted {
                            compiled_emitted = true;
                            let compiled_msg = if short_for_logs.is_empty() {
                                format!("✓ {display_name_for_logs} compiled and launched")
                            } else {
                                format!("{short_for_logs}✓ compiled and launched")
                            };
                            event_tx_logs
                                .send(AppEvent::Flutter(FlutterEvent::Log {
                                    level: LogLevel::Info,
                                    message: compiled_msg,
                                }))
                                .await
                                .ok();
                        }
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
                                serial_for_state.clone(),
                            );
                        }
                        ev
                    }
                    FlutterEvent::Stopped { .. } => {
                        event_tx_logs
                            .send(AppEvent::Device(DeviceEvent::SessionState {
                                serial: serial_for_state.clone(),
                                state: DeviceSessionState::Stopped,
                            }))
                            .await
                            .ok();
                        ev
                    }
                    other => other,
                };
                event_tx_logs.send(AppEvent::Flutter(prefixed)).await.ok();
            }

            // Daemon channel closed. Decide whether to recover or
            // give up before falling into the iOS USB replug poll.
            if !ever_started {
                event_tx_logs
                    .send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Error,
                        message: format!(
                            "{short_for_logs}flutter run exited before the app started — \
                         not a USB unplug, no recovery to do."
                        ),
                    }))
                    .await
                    .ok();
                for line in last_logs.iter().rev().take(3) {
                    event_tx_logs
                        .send(AppEvent::Flutter(FlutterEvent::Log {
                            level: LogLevel::Error,
                            message: format!("{short_for_logs}  ↳ {line}"),
                        }))
                        .await
                        .ok();
                }
                event_tx_logs.send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Info,
                    message: format!("{short_for_logs}Press `q` to exit, then run `flutter run` again with a valid `-d <device>`."),
                })).await.ok();
                return;
            }
            let now = std::time::Instant::now();
            death_history.retain(|t| now.duration_since(*t) < std::time::Duration::from_secs(30));
            death_history.push(now);
            if death_history.len() >= 3 {
                event_tx_logs
                    .send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Error,
                        message: format!(
                            "{short_for_logs}daemon died 3 times in 30s — giving up. \
                         Likely a Flutter SDK / project issue, not a USB unplug. \
                         Try `flutter clean` and look at the errors above."
                        ),
                    }))
                    .await
                    .ok();
                return;
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
            event_tx_logs
                .send(AppEvent::Device(DeviceEvent::SessionState {
                    serial: serial_for_state.clone(),
                    state: DeviceSessionState::Stopped,
                }))
                .await
                .ok();
            event_tx_logs
                .send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Warn,
                    message: format!(
                        "{short_for_logs}USB session ended — replug the cable to resume \
                     (keeping TUI alive; logs scrollable with ↑/↓)"
                    ),
                }))
                .await
                .ok();

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
                    event_tx_logs
                        .send(AppEvent::Flutter(FlutterEvent::Log {
                            level: LogLevel::Info,
                            message: format!(
                                "{short_for_logs}🔌 USB reconnected — relaunching app on device"
                            ),
                        }))
                        .await
                        .ok();
                    break;
                }
                if last_state_logged != Some(false) {
                    last_state_logged = Some(false);
                    event_tx_logs
                        .send(AppEvent::Flutter(FlutterEvent::Log {
                            level: LogLevel::Debug,
                            message: format!("{short_for_logs}waiting for USB cable…"),
                        }))
                        .await
                        .ok();
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
            // Same pass-through args as the original spawn so a respawn
            // doesn't silently drop the user's --flavor / --dart-define.
            for a in &extra_user_args_for_respawn {
                respawn_extra.push(a.as_str());
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
                    event_tx_logs
                        .send(AppEvent::Device(DeviceEvent::SessionState {
                            serial: serial_for_state.clone(),
                            state: DeviceSessionState::Connecting,
                        }))
                        .await
                        .ok();
                }
                Err(e) => {
                    event_tx_logs
                        .send(AppEvent::Flutter(FlutterEvent::Log {
                            level: LogLevel::Error,
                            message: format!("{short_for_logs}respawn failed: {e} — session ended"),
                        }))
                        .await
                        .ok();
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
    // `"[Device Name] "` (with trailing space) when multiple
    // devices are live, empty string for a solo run.
    short_name: String,
    serial: String,
) {
    tokio::spawn(async move {
        let (vm_tx, mut vm_rx) = mpsc::channel::<fl_core::VmEvent>(128);
        // Bridge VmEvents → AppEvent::Vm, tagging each with the device
        // serial so the TUI can keep per-device perf samples.
        let event_tx_bridge = event_tx.clone();
        let serial_for_bridge = serial.clone();
        tokio::spawn(async move {
            while let Some(ev) = vm_rx.recv().await {
                event_tx_bridge
                    .send(AppEvent::Vm {
                        serial: serial_for_bridge.clone(),
                        event: ev,
                    })
                    .await
                    .ok();
            }
        });

        let client = match VmServiceClient::connect(&uri, vm_tx).await {
            Ok(c) => c,
            Err(e) => {
                event_tx
                    .send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Warn,
                        message: format!("{short_name}VM Service connect failed: {e}"),
                    }))
                    .await
                    .ok();
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
            event_tx
                .send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Info,
                    message: format!("{short_name}VM Service ready"),
                }))
                .await
                .ok();
            // Memory usage is pull-only on the VM Service — poll once a
            // second and synthesize `VmEvent::GcStats` so the Performance
            // panel's memory line actually moves. We refresh the isolate
            // id on every tick because hot-restart issues a brand new
            // isolate and the old one starts returning "Sentinel /
            // Collected", which is what was previously freezing Memory at
            // 0 MB.
            let mem_client = client.clone();
            let mem_tx = event_tx.clone();
            let mem_serial = serial.clone();
            let mem_short = short_name.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut consecutive_failures: u32 = 0;
                // Track the first successful poll so we can log it once
                // (useful confirmation the Memory line should be moving)
                // plus the last reported value so we can detect a
                // stuck-at-zero scenario and surface it as a warn.
                let mut logged_first_sample = false;
                let mut warned_about_zeros = false;
                loop {
                    ticker.tick().await;
                    let iso = match mem_client.get_first_isolate_id().await {
                        Ok(id) => id,
                        Err(e) => {
                            consecutive_failures += 1;
                            if consecutive_failures == 5 {
                                mem_tx
                                    .send(AppEvent::Flutter(FlutterEvent::Log {
                                        level: LogLevel::Debug,
                                        message: format!(
                                            "{mem_short}memory poll: getVM/isolate failed ({e})"
                                        ),
                                    }))
                                    .await
                                    .ok();
                            }
                            if consecutive_failures > 60 {
                                break; // VM unreachable for a minute → give up.
                            }
                            continue;
                        }
                    };
                    match mem_client.get_memory_usage_mb(&iso).await {
                        Ok((used, total)) => {
                            consecutive_failures = 0;
                            if !logged_first_sample {
                                logged_first_sample = true;
                                mem_tx.send(AppEvent::Flutter(FlutterEvent::Log {
                                    level: LogLevel::Debug,
                                    message: format!(
                                        "{mem_short}memory poll OK: used={used:.1}MB cap={total:.1}MB"
                                    ),
                                })).await.ok();
                            }
                            // If both values stay 0 for ~10 s after the
                            // first response, something is off (VM
                            // protocol mismatch, sentinel isolate, …).
                            // Log once so the user knows the panel
                            // isn't going to start moving on its own.
                            if used == 0.0 && total == 0.0 && !warned_about_zeros {
                                warned_about_zeros = true;
                                mem_tx.send(AppEvent::Flutter(FlutterEvent::Log {
                                    level: LogLevel::Warn,
                                    message: format!(
                                        "{mem_short}memory poll returns 0/0 — heapUsage/heapCapacity may be missing in the VM Service response"
                                    ),
                                })).await.ok();
                            }
                            if mem_tx
                                .send(AppEvent::Vm {
                                    serial: mem_serial.clone(),
                                    event: fl_core::VmEvent::GcStats {
                                        used_mb: used,
                                        total_mb: total,
                                    },
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(e) => {
                            consecutive_failures += 1;
                            if consecutive_failures == 5 {
                                mem_tx
                                    .send(AppEvent::Flutter(FlutterEvent::Log {
                                        level: LogLevel::Warn,
                                        message: format!("{mem_short}memory poll failed: {e}"),
                                    }))
                                    .await
                                    .ok();
                            }
                            if consecutive_failures > 60 {
                                break;
                            }
                        }
                    }
                }
            });

            // HTTP profile poll: feeds the Network inspector panel
            // (toggled with `n`). Same idea as the memory poll, but
            // queries `ext.dart.io.getHttpProfile` and emits one
            // `AppEvent::Flutter::Log` per *new* request seen since
            // the last poll. We dedupe by request ID so a long-lived
            // request doesn't reappear every second.
            let net_client = client.clone();
            let net_tx = event_tx.clone();
            let net_short = short_name.clone();
            tokio::spawn(async move {
                use std::collections::HashSet;
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut seen: HashSet<String> = HashSet::new();
                let mut consecutive_failures: u32 = 0;
                // Track whether we've already enabled timeline
                // logging this session. The `ext.dart.io.*`
                // extensions are registered by the Dart runtime the
                // FIRST time the app touches `dart:io` — which can
                // be well after VM Service connect. Retry enabling
                // on every tick until it sticks.
                let mut timeline_enabled = false;
                // Has any HTTP request been observed yet? We only
                // emit the diagnostic "first request captured" log
                // once so the user knows the panel is actually
                // wired up.
                let mut announced = false;
                let mut logged_first_error = false;
                loop {
                    ticker.tick().await;
                    // Refresh isolate id every tick — same reasoning
                    // as the memory poll: hot restart mints a new
                    // isolate and the old id starts returning
                    // sentinels. `ext.dart.io.*` is per-isolate so
                    // we MUST pass the current one.
                    let iso = match net_client.get_first_isolate_id().await {
                        Ok(id) => id,
                        Err(_) => {
                            consecutive_failures += 1;
                            if consecutive_failures > 60 {
                                break;
                            }
                            continue;
                        }
                    };
                    if !timeline_enabled {
                        if net_client
                            .call(
                                "ext.dart.io.httpEnableTimelineLogging",
                                serde_json::json!({ "isolateId": iso, "enabled": true }),
                            )
                            .await
                            .is_ok()
                        {
                            timeline_enabled = true;
                        } else {
                            // Extension not registered yet — the
                            // Dart runtime registers `ext.dart.io.*`
                            // lazily on first `dart:io` touch.
                            // Retry next tick.
                            continue;
                        }
                    }
                    let v = match net_client
                        .call(
                            "ext.dart.io.getHttpProfile",
                            serde_json::json!({ "isolateId": iso }),
                        )
                        .await
                    {
                        Ok(v) => v,
                        Err(e) => {
                            if !logged_first_error {
                                logged_first_error = true;
                                net_tx
                                    .send(AppEvent::Flutter(FlutterEvent::Log {
                                        level: LogLevel::Debug,
                                        message: format!(
                                            "{net_short}network poll: {e} (will keep retrying)"
                                        ),
                                    }))
                                    .await
                                    .ok();
                            }
                            consecutive_failures += 1;
                            if consecutive_failures > 60 {
                                break;
                            }
                            continue;
                        }
                    };
                    consecutive_failures = 0;
                    // Different Dart versions surface the request
                    // list under slightly different keys. Try the
                    // canonical name first, then a few known aliases
                    // so the panel works across SDK channels.
                    let requests = v
                        .get("requests")
                        .or_else(|| v.get("httpRequests"))
                        .or_else(|| v.get("samples"))
                        .and_then(serde_json::Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    if !announced {
                        announced = true;
                        let keys: Vec<String> = v
                            .as_object()
                            .map(|m| m.keys().cloned().collect())
                            .unwrap_or_default();
                        net_tx
                            .send(AppEvent::Flutter(FlutterEvent::Log {
                                level: LogLevel::Debug,
                                message: format!(
                                    "{net_short}🌐 network poll OK — keys={keys:?} requests={}",
                                    requests.len()
                                ),
                            }))
                            .await
                            .ok();
                    }
                    for r in requests {
                        let id = r.get("id").map(|v| v.to_string()).unwrap_or_default();
                        if id.is_empty() || seen.contains(&id) {
                            continue;
                        }
                        // Permissive "is this done?" check. Older
                        // Dart used `isResponseComplete`; newer
                        // surfaces presence of `response.statusCode`
                        // OR a non-zero `endTime`. Accept any of
                        // them — better to show an entry than miss
                        // it. Once an entry is `seen`, dedup keeps
                        // us from double-emitting.
                        let has_status = r
                            .get("response")
                            .and_then(|x| x.get("statusCode"))
                            .is_some();
                        let has_end = r
                            .get("endTime")
                            .and_then(serde_json::Value::as_u64)
                            .map(|n| n > 0)
                            .unwrap_or(false);
                        let explicit_done = r
                            .get("isResponseComplete")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        if !(has_status || has_end || explicit_done) {
                            continue;
                        }
                        seen.insert(id.clone());
                        // Method / URL also have varying field names.
                        let method = r
                            .get("method")
                            .or_else(|| r.get("requestMethod"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("?")
                            .to_string();
                        let url = r
                            .get("uri")
                            .or_else(|| r.get("url"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let status = r
                            .get("response")
                            .and_then(|r| r.get("statusCode"))
                            .and_then(serde_json::Value::as_u64)
                            .map(|n| n as u16);
                        let start_us = r
                            .get("startTime")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0);
                        let end_us = r
                            .get("endTime")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(start_us);
                        let duration_ms = ((end_us.saturating_sub(start_us)) / 1000) as u64;
                        net_tx
                            .send(AppEvent::Device(fl_core::DeviceEvent::HttpRequest {
                                device: net_short.clone(),
                                method,
                                url,
                                status,
                                duration_ms: Some(duration_ms),
                            }))
                            .await
                            .ok();
                    }
                }
            });
        } else {
            event_tx
                .send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Warn,
                    message: format!("{short_name}VM Service: no isolate found after 10s"),
                }))
                .await
                .ok();
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
    // `s` — capture every device's current frame in parallel.
    if matches!(key, FlKey::Char('s')) {
        capture_all_screenshots(sessions, events).await;
        return;
    }

    // `d` — open this run's Flutter DevTools URL in the user's
    // browser, one tab per session. The URL is captured from the
    // daemon's `app.devTools:` log line into `session.devtools_uri`.
    if matches!(key, FlKey::Char('d')) {
        open_devtools_all(sessions, events).await;
        return;
    }

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
                                events
                                    .send(AppEvent::Flutter(FlutterEvent::Log {
                                        level: LogLevel::Info,
                                        message: format!("[{short}] {kind} requested"),
                                    }))
                                    .await
                                    .ok();
                                true
                            }
                            Err(_) => false, // Pipe broken → daemon died mid-flight.
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };

            if daemon_succeeded {
                continue;
            }

            // Daemon-less path: VM Service for hot reload, error for restart.
            if full {
                events
                    .send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Warn,
                        message: format!(
                            "[{short}] hot restart unavailable after USB unplug — \
                         use `r` for hot reload or relaunch with `fl run`"
                        ),
                    }))
                    .await
                    .ok();
                continue;
            }

            let vm = s.vm_client.lock().await.clone();
            let iso = s.isolate_id.lock().await.clone();
            let (Some(client), Some(iso)) = (vm, iso) else {
                events
                    .send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Warn,
                        message: format!("[{short}] no VM Service yet, can't reload"),
                    }))
                    .await
                    .ok();
                continue;
            };
            match client.hot_reload(&iso).await {
                Ok(_) => {
                    events
                        .send(AppEvent::Flutter(FlutterEvent::Log {
                            level: LogLevel::Info,
                            message: format!(
                                "[{short}] reload requested (via VM Service over tunnel)"
                            ),
                        }))
                        .await
                        .ok();
                }
                Err(e) => {
                    events
                        .send(AppEvent::Flutter(FlutterEvent::Log {
                            level: LogLevel::Error,
                            message: format!("[{short}] reload failed: {e:#}"),
                        }))
                        .await
                        .ok();
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
                    events
                        .send(AppEvent::Flutter(FlutterEvent::Log {
                            level: LogLevel::Warn,
                            message: format!(
                                "[{}] no live isolate — ignoring {:?}",
                                s.short_name, key
                            ),
                        }))
                        .await
                        .ok();
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
                FlKey::Char('P') => {
                    client
                        .toggle_performance_overlay(&iso, perf_overlay_on)
                        .await
                }
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
                let is_sentinel =
                    value.get("type").and_then(serde_json::Value::as_str) == Some("Sentinel");
                if is_sentinel {
                    let label = match key {
                        FlKey::Char('b') => "Theme",
                        FlKey::Char('p') => "Debug paint",
                        FlKey::Char('o') => "Platform override",
                        FlKey::Char('P') => "Performance overlay",
                        _ => "Extension",
                    };
                    events
                        .send(AppEvent::Flutter(FlutterEvent::Log {
                            level: LogLevel::Warn,
                            message: format!(
                                "[{short}] {label}: isolate was collected (likely after hot \
                             restart) — try the key again to re-target the new isolate"
                            ),
                        }))
                        .await
                        .ok();
                } else {
                    // Translate each extension's structured response into a
                    // single human-readable status line. The raw JSON
                    // (`{"method":"...","type":"_extensionType","value":"..."}`)
                    // is useless to the user — show what actually changed
                    // on the device instead.
                    let pretty = pretty_status(key, &value);
                    events
                        .send(AppEvent::Flutter(FlutterEvent::Log {
                            level: LogLevel::Info,
                            message: format!("[{short}] {pretty}"),
                        }))
                        .await
                        .ok();
                }
            }
            Err(e) => {
                events
                    .send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Error,
                        message: format!("[{short}] {key:?} -> {e}"),
                    }))
                    .await
                    .ok();
            }
        }
    }
}

/// Render the VM Service response to one of our extension keys as a
/// short human-readable status line — no raw JSON in the log.
fn pretty_status(key: FlKey, value: &serde_json::Value) -> String {
    match key {
        FlKey::Char('b') => {
            let raw = value
                .get("value")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
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
            let raw = value
                .get("value")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
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
    resolve_flutter(
        None,
        std::env::var("FLUTTER_ROOT").ok().as_deref(),
        dirs_home(),
    )
    .ok_or_else(|| anyhow!("flutter binary not found"))
}

/// Render a `BuildMode` as the lowercase label shown in the dashboard
/// header. Kept inline here (rather than as a method on `BuildMode`
/// upstream) so the user-visible wording is owned by the CLI crate.
fn mode_label(m: BuildMode) -> &'static str {
    match m {
        BuildMode::Debug => "debug",
        BuildMode::Profile => "profile",
        BuildMode::Release => "release",
    }
}

/// Press-`s` handler: capture every device's frame in parallel,
/// dump PNGs into `screenshots/<timestamp>/<device>.png`, report
/// per-device success/failure as Flutter log lines, finish with a
/// summary line.
async fn capture_all_screenshots(sessions: &[DeviceSession], events: &mpsc::Sender<AppEvent>) {
    if sessions.is_empty() {
        return;
    }
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let dir = PathBuf::from("screenshots").join(&stamp);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        events
            .send(AppEvent::Flutter(FlutterEvent::Log {
                level: LogLevel::Error,
                message: format!("📸 cannot create {}: {e}", dir.display()),
            }))
            .await
            .ok();
        return;
    }

    let mut tasks = Vec::new();
    for s in sessions {
        let serial = s.serial.clone();
        let short = s.short_name.clone();
        let display = s.display_name.clone();
        let dir = dir.clone();
        let events = events.clone();
        let vm_client = s.vm_client.lock().await.clone();
        tasks.push(tokio::spawn(async move {
            let filename = format!("{}.png", sanitize_filename(&display));
            let path = dir.join(&filename);
            match capture_one(&serial, &path, vm_client.as_ref()).await {
                Ok(method) => {
                    events
                        .send(AppEvent::Flutter(FlutterEvent::Log {
                            level: LogLevel::Info,
                            message: format!("[{short}] 📸 saved {} ({method})", path.display()),
                        }))
                        .await
                        .ok();
                    true
                }
                Err(e) => {
                    events
                        .send(AppEvent::Flutter(FlutterEvent::Log {
                            level: LogLevel::Warn,
                            message: format!("[{short}] 📸 screenshot failed: {e}"),
                        }))
                        .await
                        .ok();
                    false
                }
            }
        }));
    }
    let results = futures_util::future::join_all(tasks).await;
    let ok = results
        .into_iter()
        .filter_map(|r| r.ok())
        .filter(|b| *b)
        .count();
    events
        .send(AppEvent::Flutter(FlutterEvent::Log {
            level: LogLevel::Info,
            message: format!(
                "📸 {ok}/{} screenshots in {}",
                sessions.len(),
                dir.display()
            ),
        }))
        .await
        .ok();
}

/// Try every screenshot strategy in priority order. CRITICAL: every
/// subprocess MUST set `stdin(Stdio::null())` — otherwise the child
/// inherits the TUI's terminal stdin and reads the user's keystrokes
/// while we're in raw mode, which then bleeds into the rendered
/// dashboard as corrupted glyphs (`^[`, `^?`, etc.). The same fix
/// already lives in test_cmd / build_cmd.
async fn capture_one(
    serial: &str,
    path: &std::path::Path,
    vm_client: Option<&fl_vmservice::VmServiceClient>,
) -> anyhow::Result<&'static str> {
    use tokio::process::Command;
    let path_str = path.to_str().unwrap_or("screenshot.png");

    // 0. VM Service screenshot RPC — fastest, zero deps.
    if let Some(client) = vm_client {
        if let Ok(bytes) = client.screenshot_png().await {
            std::fs::write(path, &bytes).map_err(|e| anyhow!("write {}: {e}", path.display()))?;
            return Ok("vmservice");
        }
    }

    // 1. `flutter screenshot` — Flutter SDK's built-in.
    let flutter = Command::new("flutter")
        .args(["screenshot", "--device-id", serial, "--out", path_str])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
    if let Ok(s) = flutter {
        if s.success() && path.exists() && path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            return Ok("flutter screenshot");
        }
    }

    // 2. Android adb screencap.
    let android = Command::new("adb")
        .args(["-s", serial, "exec-out", "screencap", "-p"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await;
    if let Ok(out) = android {
        if out.status.success() && out.stdout.starts_with(&[0x89, b'P', b'N', b'G']) {
            std::fs::write(path, &out.stdout)?;
            return Ok("adb");
        }
    }

    // 3. libimobiledevice for real iOS devices.
    let ios = Command::new("idevicescreenshot")
        .args(["-u", serial, path_str])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
    if let Ok(s) = ios {
        if s.success() && path.exists() && path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            return Ok("idevicescreenshot");
        }
    }

    // 4. iOS simulators.
    let sim = Command::new("xcrun")
        .args(["simctl", "io", serial, "screenshot", path_str])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
    if let Ok(s) = sim {
        if s.success() && path.exists() && path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            return Ok("simctl");
        }
    }

    Err(anyhow!(
        "all screenshot methods failed for {serial} — VM Service unreachable, \
         flutter / adb / idevicescreenshot / simctl each declined"
    ))
}

/// Make a device display name safe to drop into a filename.
fn sanitize_filename(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '_',
            c if c.is_whitespace() => '_',
            c => c,
        })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    s.trim_matches('_').to_string()
}

/// Open the captured Flutter DevTools URL for every session in the
/// user's default browser. macOS gets `open`, everything else gets
/// `xdg-open`. Sessions whose URL hasn't been captured yet (the
/// daemon hasn't emitted `app.devTools: …` — still building, VM
/// Service not up) get a friendly warn instead of a silent no-op.
async fn open_devtools_all(sessions: &[DeviceSession], events: &mpsc::Sender<AppEvent>) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut opened = 0;
    let mut missing = 0;
    for s in sessions {
        let uri = s.devtools_uri.lock().await.clone();
        let short = s.short_name.clone();
        match uri {
            Some(u) => {
                match tokio::process::Command::new(opener)
                    .arg(&u)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await
                {
                    Ok(st) if st.success() => {
                        opened += 1;
                        events
                            .send(AppEvent::Flutter(FlutterEvent::Log {
                                level: LogLevel::Info,
                                message: format!("[{short}] 🔧 DevTools opened in browser"),
                            }))
                            .await
                            .ok();
                    }
                    _ => {
                        events
                            .send(AppEvent::Flutter(FlutterEvent::Log {
                                level: LogLevel::Warn,
                                message: format!(
                                    "[{short}] couldn't run `{opener}` — DevTools: {u}"
                                ),
                            }))
                            .await
                            .ok();
                    }
                }
            }
            None => {
                missing += 1;
                events
                    .send(AppEvent::Flutter(FlutterEvent::Log {
                        level: LogLevel::Warn,
                        message: format!(
                            "[{short}] DevTools not ready yet — still building / VM Service not up"
                        ),
                    }))
                    .await
                    .ok();
            }
        }
    }
    if opened == 0 && missing > 0 {
        events
            .send(AppEvent::Flutter(FlutterEvent::Log {
                level: LogLevel::Info,
                message: "🔧 No DevTools URL captured yet — wait for the app to start and retry"
                    .into(),
            }))
            .await
            .ok();
    }
}

/// Ask Flutter what devices it can actually see via `flutter devices
/// --machine` (JSON). Returns the set of device IDs Flutter is
/// willing to target. Returns an empty set on any failure — we then
/// fall back to the union of adb + xcrun discovery, so a broken
/// `flutter` doesn't lock the user out of the picker.
async fn flutter_known_devices(flutter: &Path) -> std::collections::HashSet<String> {
    use std::collections::HashSet;
    let out = match tokio::process::Command::new(flutter)
        .args(["devices", "--machine"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
    {
        Ok(o) if o.status.success() => o,
        _ => return HashSet::new(),
    };
    let raw = String::from_utf8_lossy(&out.stdout);
    // `flutter devices --machine` prints a JSON array, sometimes
    // preceded by a banner ("Downloading Android Maven dependencies…").
    // Locate the first `[` and parse from there.
    let start = match raw.find('[') {
        Some(i) => i,
        None => return HashSet::new(),
    };
    let v: serde_json::Value = match serde_json::from_str(&raw[start..]) {
        Ok(v) => v,
        Err(_) => return HashSet::new(),
    };
    v.as_array()
        .map(|arr| {
            arr.iter()
                // Only keep devices Flutter says it can actually
                // target. `flutter devices --machine` lists every
                // attached device (Apple Watches, half-offline
                // emulators, …) and marks the rejected ones with
                // `isSupported: false`. The `flutter run` daemon
                // then re-checks the same flag and fails with "No
                // supported devices found" — without this filter
                // the user can pick such a ghost device from the
                // picker and we have no way to predict the failure
                // until we try to spawn the run.
                .filter(|d| {
                    d.get("isSupported")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true)
                })
                .filter_map(|d| d.get("id").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn dirs_home() -> Option<&'static Path> {
    static HOME: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()))
        .as_deref()
}

#[allow(clippy::too_many_arguments)]
pub async fn run_multi(
    project: Option<PathBuf>,
    devices_arg: Vec<String>,
    all: bool,
    no_picker: bool,
    no_wifi: bool,
    no_tui: bool,
    mode: BuildMode,
    extra: Vec<String>,
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

    // Cross-reference with `flutter devices --machine` — the
    // authoritative list of devices the Flutter toolchain will
    // actually let `flutter run -d <id>` target. Without this, our
    // picker happily offers e.g. an Android emulator that adb sees
    // but Flutter's Android tooling has rejected (broken
    // ANDROID_HOME, missing SDK, …), the user picks it, and the
    // run dies with "No supported devices found". We filter it out
    // up-front so the picker only shows runnable targets.
    // Quietly drop devices Flutter wouldn't run on (Apple Watches,
    // unsupported iPads, half-broken emulators, …). No stderr noise:
    // the picker showing only runnable devices is enough signal.
    let flutter_known = flutter_known_devices(&flutter).await;
    if !flutter_known.is_empty() {
        all_devices.retain(|d| flutter_known.contains(&d.serial));
    }

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
        all_devices
            .first()
            .map(|d| vec![d.serial.clone()])
            .unwrap_or_default()
    } else {
        // Picker cancelled (`q`/`Esc`) is a clean exit, not an
        // error. Returning an empty Vec lets the check below
        // short-circuit without bubbling a panicky "Error:" message
        // to the user.
        match run_picker(&all_devices).await {
            Ok(v) => v,
            Err(_) => Vec::new(),
        }
    };

    if chosen.is_empty() {
        // Either no devices at all, or the user backed out of the
        // picker. Exit silently — the picker UI already gave the
        // visual feedback ("q quit" footer), no need for an extra
        // red Error line.
        return Ok(());
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
            event_tx
                .send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Warn,
                    message: format!("mDNS browser unavailable: {e} — Wi-Fi takeover disabled"),
                }))
                .await
                .ok();
            None
        }
    };

    // Pre-register EVERY chosen session in `Connecting` state up
    // front, before we start spawning sessions sequentially. Without
    // this, a fast device (e.g. Android already-installed app) can
    // race past `AppStarted` before the next device's `spawn_session`
    // has even sent its initial `SessionState::Connecting` — the TUI
    // would then think "all sessions are Ready" with only one
    // registered, freeze the chronometer on green ✓ prematurely, and
    // show "All devices compiled" while iPhone is still building.
    for serial in &chosen {
        event_tx
            .send(AppEvent::Device(DeviceEvent::SessionState {
                serial: serial.clone(),
                state: DeviceSessionState::Connecting,
            }))
            .await
            .ok();
    }

    let mut sessions: Vec<DeviceSession> = Vec::new();
    for serial in &chosen {
        // Pick the USB serial we should pre-pair for Wi-Fi takeover.
        // Skip Android emulators — their `wlan0` is a NAT-local
        // 10.0.2.x address unreachable from the host, so `adb
        // connect` would just hang and surface a confusing "Wi-Fi
        // pre-pair failed" error for a setup that doesn't need it.
        let usb_pair = all_devices
            .iter()
            .find(|d| {
                d.serial == *serial
                    && matches!(d.connection, fl_core::ConnectionKind::Usb)
                    && !d.serial.starts_with("emulator-")
                    && (d.platform.as_deref() == Some("android") || d.platform.is_none())
            })
            .map(|d| d.serial.clone());
        // iOS-only optimisation: when the user picked a wired iPhone,
        // force `flutter run --device-connection attached` so the
        // daemon uses the coredevice USB tunnel instead of silently
        // falling back to the slow Bonjour wireless transport on
        // iOS 17+. For Android (and especially Android emulators)
        // we must NOT pass that flag — Flutter interprets it as
        // "exclude non-attached devices" and the emulator gets
        // filtered out, surfacing as "No supported devices found
        // with name or id matching 'emulator-5554'".
        let device_info = all_devices.iter().find(|d| d.serial == *serial);
        let prefer_attached = device_info
            .map(|d| {
                matches!(d.connection, fl_core::ConnectionKind::Usb)
                    && d.platform.as_deref() == Some("ios")
            })
            .unwrap_or(false);
        // Pull the human-readable name from whichever discovery
        // source had it (xcrun `deviceProperties.name` for iOS,
        // `flutter devices --machine` for emulators, `model:` field
        // from `adb devices -l` for Android USB). Falls back to the
        // device's `model` if `name` is just the serial.
        let discovered_name = device_info.map(|d| {
            if d.name != d.serial && !d.name.is_empty() {
                d.name.clone()
            } else {
                d.model.clone().unwrap_or_else(|| d.serial.clone())
            }
        });
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
            extra.clone(),
            chosen.len() > 1,
            discovered_name,
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
        let mut state = AppState::new(app_name, mode_label(mode).into());
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

    let app_name = project
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app")
        .to_string();
    let mut state = AppState::new(app_name, mode_label(mode).into());
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
                    FlKey::Char('r')
                        | FlKey::Char('R')
                        | FlKey::Char('b')
                        | FlKey::Char('p')
                        | FlKey::Char('o')
                        | FlKey::Char('P')
                        | FlKey::Char('s')
                        | FlKey::Char('d')
                ) {
                    let bs = brightness.load(std::sync::atomic::Ordering::Relaxed);
                    let brightness_value: Option<bool> = match bs {
                        fl_tui::app::BRIGHTNESS_LIGHT => Some(false),
                        fl_tui::app::BRIGHTNESS_DARK => Some(true),
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
    let ts = format!(
        "{:>4}.{:01}s",
        elapsed.as_secs(),
        elapsed.subsec_millis() / 100
    );
    let line = match ev {
        AppEvent::Flutter(FlutterEvent::Log { level, message }) => {
            let tag = match level {
                LogLevel::Error => "\x1b[1;31mERROR\x1b[0m",
                LogLevel::Warn => "\x1b[1;33mWARN \x1b[0m",
                LogLevel::Info => "\x1b[1;36mINFO \x1b[0m",
                LogLevel::Debug => "\x1b[90mDEBUG\x1b[0m",
                LogLevel::Trace => "\x1b[90mTRACE\x1b[0m",
            };
            format!("\x1b[90m{ts}\x1b[0m {tag} {message}")
        }
        AppEvent::Flutter(FlutterEvent::AppStarted {
            app_id,
            vm_service_uri,
        }) => {
            format!("\x1b[90m{ts}\x1b[0m \x1b[1;32mSTART\x1b[0m app={app_id} vm={vm_service_uri}")
        }
        AppEvent::Flutter(FlutterEvent::Stopped { exit_code }) => {
            format!("\x1b[90m{ts}\x1b[0m \x1b[1;31mSTOP \x1b[0m exit_code={exit_code:?}")
        }
        AppEvent::Flutter(FlutterEvent::Progress {
            id,
            message,
            finished,
        }) => {
            let mark = if *finished { "✓" } else { "…" };
            format!("\x1b[90m{ts}\x1b[0m \x1b[1;34mPROG \x1b[0m {mark} [{id}] {message}")
        }
        AppEvent::Flutter(other) => {
            format!("\x1b[90m{ts}\x1b[0m \x1b[1;36mFLU  \x1b[0m {other:?}")
        }
        AppEvent::Device(d) => {
            format!("\x1b[90m{ts}\x1b[0m \x1b[1;35mDEV  \x1b[0m {d:?}")
        }
        AppEvent::Vm { serial, event } => {
            format!("\x1b[90m{ts}\x1b[0m \x1b[1;34mVM   \x1b[0m [{serial}] {event:?}")
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
                let _ = tokio::time::timeout(std::time::Duration::from_millis(500), d.send_quit())
                    .await;
            }
        }
    });
    futures_util::future::join_all(quits).await;

    let waits = sessions.iter().map(|s| {
        let daemon = s.daemon.clone();
        async move {
            let mut guard = daemon.lock().await;
            if let Some(d) = guard.as_mut() {
                if tokio::time::timeout(std::time::Duration::from_secs(1), d.wait())
                    .await
                    .is_err()
                {
                    let _ = d.kill().await;
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_millis(500), d.wait()).await;
                }
            }
        }
    });
    futures_util::future::join_all(waits).await;
}

pub async fn run_picker(devices: &[fl_core::Device]) -> anyhow::Result<Vec<String>> {
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
