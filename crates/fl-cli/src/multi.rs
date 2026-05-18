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

#[allow(dead_code)]
pub struct DeviceSession {
    pub serial: String,
    pub short_name: String,
    pub display_name: String,
    pub daemon: Arc<Mutex<Option<FlutterDaemon>>>,
    pub vm_client: Arc<Mutex<Option<VmServiceClient>>>,
    pub isolate_id: Arc<Mutex<Option<String>>>,
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

    let (flutter_tx, mut flutter_rx) = mpsc::channel::<FlutterEvent>(64);
    let mode_flag = mode.flutter_flag();
    let extra: Vec<&str> = if matches!(mode, BuildMode::Debug) { Vec::new() } else { vec![mode_flag] };
    let daemon = FlutterDaemon::spawn(flutter, project, &final_target, &extra, flutter_tx).await?;
    *session.daemon.lock().await = Some(daemon);

    let short_for_logs = session.short_name.clone();
    let serial_for_state = session.serial.clone();
    let event_tx_logs = event_tx.clone();
    tokio::spawn(async move {
        while let Some(ev) = flutter_rx.recv().await {
            let prefixed = match ev {
                FlutterEvent::Log { level, message } => FlutterEvent::Log {
                    level,
                    message: format!("[{short_for_logs}] {message}"),
                },
                FlutterEvent::AppStarted { .. } => {
                    event_tx_logs.send(AppEvent::Device(DeviceEvent::SessionState {
                        serial: serial_for_state.clone(),
                        state: DeviceSessionState::Ready,
                    })).await.ok();
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
        // The daemon's stdout/stderr pipes have closed — the `flutter` process
        // exited. Inform the TUI so it can quit (or, in multi-device mode, mark
        // this session done while the others keep running).
        event_tx_logs.send(AppEvent::Device(DeviceEvent::SessionState {
            serial: serial_for_state.clone(),
            state: DeviceSessionState::Stopped,
        })).await.ok();
        event_tx_logs.send(AppEvent::Flutter(FlutterEvent::Log {
            level: LogLevel::Info,
            message: format!("[{short_for_logs}] flutter daemon exited"),
        })).await.ok();
    });

    Ok(session)
}

#[allow(dead_code)]
pub async fn broadcast_key(key: FlKey, sessions: &[DeviceSession], events: &mpsc::Sender<AppEvent>) {
    let mut futures = Vec::new();
    for s in sessions {
        let vm = s.vm_client.lock().await.clone();
        let iso = s.isolate_id.lock().await.clone();
        let (Some(client), Some(iso)) = (vm, iso) else { continue };
        let short = s.short_name.clone();
        let key_copy = key;
        futures.push(async move {
            let res = match key_copy {
                FlKey::Char('r') => client.hot_reload(&iso).await,
                FlKey::Char('R') => client.hot_restart(&iso).await,
                FlKey::Char('b') => client.toggle_brightness(&iso, true).await,
                FlKey::Char('p') => client.toggle_debug_paint(&iso, true).await,
                FlKey::Char('o') => client.toggle_platform(&iso, false).await,
                FlKey::Char('P') => client.toggle_performance_overlay(&iso, true).await,
                _ => return None,
            };
            Some((short, res.err().map(|e| e.to_string())))
        });
    }
    let results = futures_util::future::join_all(futures).await;
    for outcome in results.into_iter().flatten() {
        let (short, err) = outcome;
        match err {
            None if matches!(key, FlKey::Char('r')) => {
                events.send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Info,
                    message: format!("[{short}] reload OK"),
                })).await.ok();
            }
            Some(e) => {
                events.send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Error,
                    message: format!("[{short}] {key:?} -> {e}"),
                })).await.ok();
            }
            _ => {}
        }
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
    mode: BuildMode,
) -> anyhow::Result<()> {
    let project = project.unwrap_or_else(|| std::env::current_dir().unwrap());
    let flutter = resolve_flutter_path()?;
    let runner = Arc::new(TokioRunner);

    let listed = runner.run("adb", &["devices", "-l"]).await?;
    let mut all_devices = parse_devices_l(&listed.stdout);
    let xcrun = fl_ios::Xcrun::new(TokioRunner);
    all_devices.extend(fl_ios::list_apple_devices(&xcrun).await);

    let headless = std::env::var_os("FL_HEADLESS").is_some();
    let chosen: Vec<String> = if !devices_arg.is_empty() {
        devices_arg
    } else if all {
        if all_devices.is_empty() {
            return Err(anyhow!("--all specified but no devices attached"));
        }
        all_devices.iter().map(|d| d.serial.clone()).collect()
    } else if all_devices.len() <= 1 || no_picker || headless {
        all_devices.first().map(|d| vec![d.serial.clone()]).unwrap_or_default()
    } else {
        run_picker(&all_devices).await?
    };

    if chosen.is_empty() {
        return Err(anyhow!("no devices to run on"));
    }

    let (event_tx, mut event_rx) = mpsc::channel::<AppEvent>(256);
    let mut sessions: Vec<DeviceSession> = Vec::new();
    for serial in &chosen {
        let usb_pair = all_devices
            .iter()
            .find(|d| d.serial == *serial
                  && matches!(d.connection, fl_core::ConnectionKind::Usb)
                  && (d.platform.as_deref() == Some("android") || d.platform.is_none()))
            .map(|d| d.serial.clone());
        let s = spawn_session(
            runner.clone(),
            &flutter,
            &project,
            serial.clone(),
            usb_pair,
            no_wifi,
            mode,
            event_tx.clone(),
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

    if headless {
        let app_name = project
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("app")
            .to_string();
        let mut state = AppState::new(app_name, "debug".into());
        while let Some(ev) = event_rx.recv().await {
            println!("{ev:?}");
            state.apply(ev);
            if !state.active_sessions.is_empty()
                && state.active_sessions.iter().all(|s| matches!(s.state, DeviceSessionState::Stopped)) {
                break;
            }
        }
        return Ok(());
    }

    let app_name = project.file_name().and_then(|n| n.to_str()).unwrap_or("app").to_string();
    let mut state = AppState::new(app_name, "debug".into());
    let mut tui = TuiRunner::init()?;
    let (keys_tx, _keys_rx) = mpsc::channel::<FlKey>(1);
    let result = tui.run(&mut state, &mut event_rx, keys_tx).await;

    // Restore the terminal IMMEDIATELY so the user gets their shell back.
    // Daemon shutdown then runs against a normal terminal, with shorter timeouts
    // and parallel execution.
    let _ = tui.restore();
    dump_exit_summary(&state);
    eprintln!("Shutting down…");
    shutdown_sessions_fast(&sessions).await;
    result
}

/// Print everything we know after exiting the TUI so the user can see why
/// `fl run` ended — full log buffer + final session states.
fn dump_exit_summary(state: &AppState) {
    eprintln!();
    eprintln!("=== Sessions on exit ===");
    if state.active_sessions.is_empty() {
        eprintln!("(none)");
    } else {
        for s in &state.active_sessions {
            eprintln!(
                "  [{}] {} ({}) — {:?}",
                s.short_name, s.display_name, s.serial, s.state
            );
        }
    }
    eprintln!();
    eprintln!("=== Last {} log lines ===", state.logs.len());
    for line in &state.logs {
        let lvl = match line.level {
            fl_core::LogLevel::Trace => "TRACE",
            fl_core::LogLevel::Debug => "DEBUG",
            fl_core::LogLevel::Info  => "INFO ",
            fl_core::LogLevel::Warn  => "WARN ",
            fl_core::LogLevel::Error => "ERROR",
        };
        eprintln!("{lvl} {}", line.message);
    }
    eprintln!();
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
    let mut tui = TuiRunner::init()?;
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
