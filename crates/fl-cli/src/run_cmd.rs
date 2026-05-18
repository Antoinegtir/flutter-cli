//! `fl run` — auto-pair USB→WiFi, launch flutter, render dashboard, route keys to VM Service.

use anyhow::anyhow;
use fl_adb::{parse_devices_l, pre_pair_wifi, track_devices, CommandRunner, TokioRunner};
use fl_core::{AppEvent, DeviceEvent, FlutterEvent, KeyEvent as FlKey, LogLevel};
use fl_flutter::{resolve_flutter, FlutterDaemon};
use fl_tui::{AppState, TuiRunner};
use fl_vmservice::VmServiceClient;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::Mutex;

pub async fn run(project: Option<PathBuf>, device: Option<String>, no_wifi: bool, mode: fl_core::BuildMode) -> anyhow::Result<()> {
    let project = project.unwrap_or_else(|| std::env::current_dir().unwrap());
    let app_name = project
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app")
        .to_string();
    let flutter = resolve_flutter(None, std::env::var("FLUTTER_ROOT").ok().as_deref(), dirs_home().as_deref())
        .ok_or_else(|| anyhow!("flutter binary not found — set FLUTTER_ROOT or install Flutter"))?;

    let (event_tx, mut event_rx) = mpsc::channel::<AppEvent>(256);
    let (keys_tx, mut keys_rx) = mpsc::channel::<FlKey>(64);

    let runner_arc = std::sync::Arc::new(TokioRunner);
    let (target_serial, usb_serial_opt, paired_target) = match device {
        Some(s) => (s, None, None),
        None => {
            let out = runner_arc.run("adb", &["devices", "-l"]).await?;
            let list = parse_devices_l(&out.stdout);
            let usb = list.iter().find(|d| matches!(d.connection, fl_core::ConnectionKind::Usb));
            match (usb, no_wifi) {
                (Some(d), false) => match pre_pair_wifi(runner_arc.as_ref(), &d.serial, 5555).await {
                    Ok(t) => {
                        event_tx.send(AppEvent::Device(DeviceEvent::WifiPaired {
                            serial: d.serial.clone(),
                            ip: t.ip.clone(),
                            port: t.port,
                        })).await.ok();
                        (t.serial(), Some(d.serial.clone()), Some(t))
                    }
                    Err(e) => {
                        event_tx.send(AppEvent::Device(DeviceEvent::Error(format!("pre-pair failed: {e}")))).await.ok();
                        (d.serial.clone(), Some(d.serial.clone()), None)
                    }
                },
                (Some(d), true) => (d.serial.clone(), Some(d.serial.clone()), None),
                (None, _) => (
                    list.first()
                        .map(|d| d.serial.clone())
                        .ok_or_else(|| anyhow!("no attached device"))?,
                    None,
                    None,
                ),
            }
        }
    };

    // Resolve device name for mDNS filtering (best-effort).
    let device_name = if let Some(serial) = usb_serial_opt.as_deref() {
        runner_arc
            .run("adb", &["-s", serial, "shell", "getprop", "ro.product.model"])
            .await
            .ok()
            .map(|o| o.stdout.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| target_serial.clone())
    } else {
        target_serial.clone()
    };

    // Spawn the ReconnectManager only when we have a WifiTarget.
    let reconnect_input: Option<tokio::sync::mpsc::Sender<fl_adb::Input>> = if let Some(target) = paired_target.clone() {
        let setup = fl_adb::ManagerSetup { target, device_name: device_name.clone() };
        let (rc_out_tx, mut rc_out_rx) = tokio::sync::mpsc::channel::<DeviceEvent>(64);
        let handle = fl_adb::spawn(setup, runner_arc.clone(), rc_out_tx);

        // Forward Reconnect outputs to the global event channel.
        let tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = rc_out_rx.recv().await {
                tx.send(AppEvent::Device(ev)).await.ok();
            }
        });

        // Spawn mDNS listener (silently disable if it fails to start).
        match fl_adb::mdns::spawn(device_name.clone(), handle.input_tx.clone()) {
            Ok(_join) => {}
            Err(e) => tracing::warn!("mDNS listener failed to start: {e}"),
        }

        Some(handle.input_tx)
    } else {
        None
    };

    {
        let tx = event_tx.clone();
        let reconnect_tx = reconnect_input.clone();
        tokio::spawn(async move {
            let (dev_tx, mut dev_rx) = mpsc::channel(32);
            tokio::spawn(async move {
                if let Err(e) = track_devices(dev_tx).await {
                    tracing::warn!("track-devices loop ended: {e}");
                }
            });
            while let Some(ev) = dev_rx.recv().await {
                if let Some(rcx) = reconnect_tx.as_ref() {
                    match &ev {
                        DeviceEvent::Lost { serial } => {
                            rcx.send(fl_adb::Input::DeviceLost { serial: serial.clone() }).await.ok();
                        }
                        DeviceEvent::Discovered(d) => {
                            rcx.send(fl_adb::Input::DeviceDiscovered { serial: d.serial.clone() }).await.ok();
                        }
                        _ => {}
                    }
                }
                tx.send(AppEvent::Device(ev)).await.ok();
            }
        });
    }

    let (flutter_tx, mut flutter_rx) = mpsc::channel::<FlutterEvent>(64);
    let mode_flag = mode.flutter_flag();
    let extra: Vec<&str> = if matches!(mode, fl_core::BuildMode::Debug) {
        Vec::new()  // debug is implicit for `flutter run`
    } else {
        vec![mode_flag]
    };
    let _daemon: Arc<Mutex<Option<FlutterDaemon>>> = Arc::new(Mutex::new(Some(
        FlutterDaemon::spawn(&flutter, &project, &target_serial, &extra, flutter_tx).await?,
    )));

    {
        let tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = flutter_rx.recv().await {
                tx.send(AppEvent::Flutter(ev)).await.ok();
            }
        });
    }

    let vm_client: Arc<Mutex<Option<VmServiceClient>>> = Arc::new(Mutex::new(None));
    let isolate_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let mut state = AppState::new(app_name, "debug".into());

    if std::env::var_os("FL_HEADLESS").is_some() {
        return run_headless(state, event_rx).await;
    }

    let mut tui = TuiRunner::init()?;
    let (tui_tx, tui_rx) = mpsc::channel::<AppEvent>(256);
    {
        let tx = tui_tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = event_rx.recv().await {
                tx.send(ev).await.ok();
            }
        });
    }

    let vm_client_for_key = vm_client.clone();
    let isolate_for_key = isolate_id.clone();
    let event_tx_for_key = event_tx.clone();
    tokio::spawn(async move {
        while let Some(k) = keys_rx.recv().await {
            handle_key(k, &vm_client_for_key, &isolate_for_key, &event_tx_for_key).await;
        }
    });

    let vm_client_for_started = vm_client.clone();
    let isolate_for_started = isolate_id.clone();
    let event_tx_for_started = event_tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let vm = vm_client_for_started.lock().await;
            if vm.is_some() {
                let id = match vm.as_ref().unwrap().get_first_isolate_id().await {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                drop(vm);
                *isolate_for_started.lock().await = Some(id);
                event_tx_for_started.send(AppEvent::Flutter(FlutterEvent::Log {
                    level: LogLevel::Debug,
                    message: "VM Service: isolate ready".into(),
                })).await.ok();
                break;
            }
        }
    });

    let vm_client_for_attach = vm_client.clone();
    let event_tx_for_attach = event_tx.clone();
    let mut tui_rx_drainer = tui_rx;
    let result = tui.run(&mut state, &mut tui_rx_drainer, keys_tx.clone()).await;

    {
        if let Some(daemon) = _daemon.lock().await.as_mut() {
            let _ = daemon.send_quit().await;
            let _ = daemon.wait().await;
        }
    }
    let _ = tui.restore();

    // Suppress unused warnings on attach hooks (used in future iterations).
    let _ = (vm_client_for_attach, event_tx_for_attach);
    result
}

async fn handle_key(
    k: FlKey,
    vm: &Arc<Mutex<Option<VmServiceClient>>>,
    isolate: &Arc<Mutex<Option<String>>>,
    events: &mpsc::Sender<AppEvent>,
) {
    let client = vm.lock().await.clone();
    let iso = isolate.lock().await.clone();
    let (Some(client), Some(iso)) = (client, iso) else {
        return;
    };
    let res = match k {
        FlKey::Char('r') => client.hot_reload(&iso).await,
        FlKey::Char('R') => client.hot_restart(&iso).await,
        FlKey::Char('b') => client.toggle_brightness(&iso, true).await,
        FlKey::Char('p') => client.toggle_debug_paint(&iso, true).await,
        FlKey::Char('o') => client.toggle_platform(&iso, false).await,
        FlKey::Char('P') => client.toggle_performance_overlay(&iso, true).await,
        _ => return,
    };
    if let Err(e) = res {
        events.send(AppEvent::Flutter(FlutterEvent::Log {
            level: LogLevel::Error,
            message: format!("key {k:?} -> {e}"),
        })).await.ok();
    } else if matches!(k, FlKey::Char('r')) {
        events.send(AppEvent::Flutter(FlutterEvent::Log {
            level: LogLevel::Info,
            message: "hot reload OK".into(),
        })).await.ok();
    }
}

fn dirs_home() -> Option<PathBuf> {
    directories::UserDirs::new().map(|u| u.home_dir().to_path_buf())
}

async fn run_headless(mut state: AppState, mut rx: mpsc::Receiver<AppEvent>) -> anyhow::Result<()> {
    while let Some(ev) = rx.recv().await {
        let line = match &ev {
            AppEvent::Device(d) => format!("DEV {d:?}"),
            AppEvent::Flutter(f) => format!("FLU {f:?}"),
            AppEvent::Vm(v) => format!("VM  {v:?}"),
            AppEvent::Key(k) => format!("KEY {k:?}"),
            AppEvent::Tick => continue,
        };
        println!("{line}");
        state.apply(ev);
        if state.quitting { break; }
    }
    Ok(())
}
