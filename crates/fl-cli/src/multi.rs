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
    });

    Ok(session)
}

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

// Suppress unused-warning while Task 7 wires this module in.
#[allow(dead_code)]
fn _multi_module_marker() {
    let _ = parse_devices_l;
    let _ = track_devices;
    let _ = TokioRunner;
    let _ = AppState::new;
    let _ = TuiRunner::init;
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
