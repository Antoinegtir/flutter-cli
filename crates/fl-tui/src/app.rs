//! State mutated by `AppEvent`s and read by the renderer.

use fl_core::{AppEvent, Device, DeviceEvent, FlutterEvent, LogLevel, VmEvent};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const LOG_RING: usize = 5_000;
const FPS_RING: usize = 60;
const MEM_RING: usize = 60;

#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug)]
pub struct AppState {
    pub app_name: String,
    pub mode: String,
    pub active_device: Option<Device>,
    pub backup_device: Option<Device>,
    pub logs: VecDeque<LogLine>,
    pub log_filter: Option<String>,
    pub fps_samples: VecDeque<f32>,
    pub frame_ui_ms: f32,
    pub frame_raster_ms: f32,
    pub mem_samples: VecDeque<f32>,
    pub rebuilds_per_sec: u32,
    pub vm_service_uri: Option<String>,
    pub vm_connected: bool,
    pub banner: Option<Banner>,
    pub last_reload_at: Option<Instant>,
    pub quitting: bool,
}

#[derive(Debug, Clone)]
pub struct Banner {
    pub kind: BannerKind,
    pub message: String,
    pub shown_at: Instant,
    /// `None` means the banner stays on screen until explicitly cleared.
    pub duration: Option<Duration>,
}

#[derive(Debug, Clone, Copy)]
pub enum BannerKind {
    Info,
    Warn,
    Error,
    Success,
}

impl AppState {
    pub fn new(app_name: String, mode: String) -> Self {
        Self {
            app_name,
            mode,
            active_device: None,
            backup_device: None,
            logs: VecDeque::with_capacity(LOG_RING),
            log_filter: None,
            fps_samples: VecDeque::with_capacity(FPS_RING),
            frame_ui_ms: 0.0,
            frame_raster_ms: 0.0,
            mem_samples: VecDeque::with_capacity(MEM_RING),
            rebuilds_per_sec: 0,
            vm_service_uri: None,
            vm_connected: false,
            banner: None,
            last_reload_at: None,
            quitting: false,
        }
    }

    pub fn apply(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Device(d) => self.apply_device(d),
            AppEvent::Flutter(f) => self.apply_flutter(f),
            AppEvent::Vm(v) => self.apply_vm(v),
            AppEvent::Key(_) | AppEvent::Tick => {}
        }
        self.expire_banner();
    }

    fn apply_device(&mut self, ev: DeviceEvent) {
        match ev {
            DeviceEvent::Discovered(d) => {
                if self.active_device.is_none() {
                    self.active_device = Some(d);
                } else {
                    self.backup_device = Some(d);
                }
            }
            DeviceEvent::Lost { serial } => {
                if self.active_device.as_ref().is_some_and(|d| d.serial == serial) {
                    self.active_device = self.backup_device.take();
                } else if self.backup_device.as_ref().is_some_and(|d| d.serial == serial) {
                    self.backup_device = None;
                }
            }
            DeviceEvent::UsbDisconnected { serial: _ } => {
                self.show_banner(BannerKind::Info, "USB déconnecté — WiFi prend le relais");
            }
            DeviceEvent::WifiPaired { .. } => {
                self.show_banner(BannerKind::Success, "WiFi pairing OK");
            }
            DeviceEvent::WifiReconnecting { attempt } => {
                self.show_persistent_banner(
                    BannerKind::Warn,
                    &format!("Reconnecting WiFi (#{attempt})"),
                );
            }
            DeviceEvent::WifiReconnected => {
                self.clear_persistent_banner();
                self.show_banner(BannerKind::Success, "WiFi reconnected");
            }
            DeviceEvent::IpChanged { new_ip, .. } => {
                self.show_banner(BannerKind::Success, &format!("New IP: {new_ip}"));
                if let Some(d) = self.active_device.as_mut() {
                    d.ip = Some(new_ip.clone());
                }
            }
            DeviceEvent::Error(msg) => {
                self.show_banner(BannerKind::Error, &msg);
            }
        }
    }

    fn apply_flutter(&mut self, ev: FlutterEvent) {
        match ev {
            FlutterEvent::DaemonReady => self.push_log(LogLevel::Debug, "daemon ready".into()),
            FlutterEvent::AppStarted { vm_service_uri, .. } => {
                self.vm_service_uri = Some(vm_service_uri);
            }
            FlutterEvent::Log { level, message } => self.push_log(level, message),
            FlutterEvent::Progress { message, finished, .. } => {
                if !message.is_empty() {
                    self.push_log(
                        if finished { LogLevel::Info } else { LogLevel::Debug },
                        message,
                    );
                }
            }
            FlutterEvent::Stopped { exit_code } => {
                self.push_log(LogLevel::Info, format!("flutter exited ({:?})", exit_code));
                self.quitting = true;
            }
            FlutterEvent::Error(msg) => self.push_log(LogLevel::Error, msg),
        }
    }

    fn apply_vm(&mut self, ev: VmEvent) {
        match ev {
            VmEvent::Connected => self.vm_connected = true,
            VmEvent::Disconnected => self.vm_connected = false,
            VmEvent::Stdout(s) => self.push_log(LogLevel::Info, s.trim_end().into()),
            VmEvent::Stderr(s) => self.push_log(LogLevel::Error, s.trim_end().into()),
            VmEvent::FrameTiming { ui_micros, raster_micros } => {
                let total_ms = (ui_micros + raster_micros) as f32 / 1000.0;
                let fps = if total_ms > 0.0 { 1000.0 / total_ms } else { 0.0 };
                push_capped(&mut self.fps_samples, fps.clamp(0.0, 120.0), FPS_RING);
                self.frame_ui_ms = ui_micros as f32 / 1000.0;
                self.frame_raster_ms = raster_micros as f32 / 1000.0;
            }
            VmEvent::GcStats { used_mb, total_mb: _ } => {
                push_capped(&mut self.mem_samples, used_mb as f32, MEM_RING);
            }
            VmEvent::IsolateEvent(_) | VmEvent::ExtensionResult { .. } => {}
        }
    }

    pub fn push_log(&mut self, level: LogLevel, message: String) {
        if self.logs.len() >= LOG_RING {
            self.logs.pop_front();
        }
        self.logs.push_back(LogLine { level, message });
    }

    pub fn show_banner(&mut self, kind: BannerKind, message: &str) {
        self.banner = Some(Banner {
            kind,
            message: message.into(),
            shown_at: Instant::now(),
            duration: Some(Duration::from_millis(3000)),
        });
    }

    pub fn show_persistent_banner(&mut self, kind: BannerKind, message: &str) {
        self.banner = Some(Banner {
            kind,
            message: message.into(),
            shown_at: Instant::now(),
            duration: None,
        });
    }

    pub fn clear_persistent_banner(&mut self) {
        if let Some(b) = &self.banner {
            if b.duration.is_none() {
                self.banner = None;
            }
        }
    }

    fn expire_banner(&mut self) {
        if let Some(b) = &self.banner {
            if let Some(d) = b.duration {
                if b.shown_at.elapsed() >= d {
                    self.banner = None;
                }
            }
        }
    }

    pub fn flash_reload(&mut self) {
        self.last_reload_at = Some(Instant::now());
    }

    pub fn reload_flash_alpha(&self) -> f32 {
        match self.last_reload_at {
            Some(t) => {
                let ms = t.elapsed().as_millis() as f32;
                (1.0 - ms / 200.0).clamp(0.0, 1.0)
            }
            None => 0.0,
        }
    }
}

fn push_capped<T>(buf: &mut VecDeque<T>, item: T, cap: usize) {
    if buf.len() >= cap {
        buf.pop_front();
    }
    buf.push_back(item);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fl_core::{ConnectionKind, DeviceState};

    fn dev(serial: &str, c: ConnectionKind) -> Device {
        Device {
            serial: serial.into(),
            name: serial.into(),
            model: None,
            connection: c,
            state: DeviceState::Online,
            ip: None,
            android_version: None,
            battery: None,
        }
    }

    #[test]
    fn discovered_device_becomes_active_when_no_other() {
        let mut s = AppState::new("app".into(), "debug".into());
        s.apply(AppEvent::Device(DeviceEvent::Discovered(dev("A", ConnectionKind::Usb))));
        assert_eq!(s.active_device.as_ref().unwrap().serial, "A");
    }

    #[test]
    fn second_discovered_becomes_backup() {
        let mut s = AppState::new("app".into(), "debug".into());
        s.apply(AppEvent::Device(DeviceEvent::Discovered(dev("A", ConnectionKind::Wifi))));
        s.apply(AppEvent::Device(DeviceEvent::Discovered(dev("B", ConnectionKind::Usb))));
        assert_eq!(s.active_device.as_ref().unwrap().serial, "A");
        assert_eq!(s.backup_device.as_ref().unwrap().serial, "B");
    }

    #[test]
    fn frame_timing_pushes_fps_sample() {
        let mut s = AppState::new("app".into(), "debug".into());
        s.apply(AppEvent::Vm(VmEvent::FrameTiming { ui_micros: 5_000, raster_micros: 11_000 }));
        assert_eq!(s.fps_samples.len(), 1);
        let fps = *s.fps_samples.front().unwrap();
        assert!((fps - 62.5).abs() < 0.5);
    }

    #[test]
    fn logs_are_ring_buffered() {
        let mut s = AppState::new("a".into(), "d".into());
        for i in 0..(LOG_RING + 100) {
            s.push_log(LogLevel::Info, format!("{i}"));
        }
        assert_eq!(s.logs.len(), LOG_RING);
        assert_eq!(s.logs.front().unwrap().message, "100");
    }

    #[test]
    fn usb_disconnected_shows_a_banner() {
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(AppEvent::Device(DeviceEvent::UsbDisconnected { serial: "X".into() }));
        assert!(s.banner.is_some());
    }

    #[test]
    fn persistent_banner_does_not_expire() {
        let mut s = AppState::new("a".into(), "d".into());
        s.show_persistent_banner(BannerKind::Warn, "stays put");
        s.apply(AppEvent::Tick);
        s.apply(AppEvent::Tick);
        assert!(s.banner.is_some());
        assert!(s.banner.as_ref().unwrap().duration.is_none());
    }

    #[test]
    fn clear_persistent_banner_only_clears_persistent() {
        let mut s = AppState::new("a".into(), "d".into());
        s.show_banner(BannerKind::Info, "transient");
        s.clear_persistent_banner();
        assert!(s.banner.is_some(), "transient banner should survive clear_persistent_banner");

        s.show_persistent_banner(BannerKind::Warn, "sticky");
        s.clear_persistent_banner();
        assert!(s.banner.is_none(), "persistent banner should be cleared");
    }

    #[test]
    fn wifi_reconnecting_sets_persistent_warn_banner() {
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(AppEvent::Device(DeviceEvent::WifiReconnecting { attempt: 3 }));
        let b = s.banner.as_ref().expect("banner present");
        assert!(matches!(b.kind, BannerKind::Warn));
        assert!(b.duration.is_none(), "should be persistent");
        assert!(b.message.contains("#3"));
    }

    #[test]
    fn wifi_reconnected_clears_persistent_and_shows_success() {
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(AppEvent::Device(DeviceEvent::WifiReconnecting { attempt: 1 }));
        s.apply(AppEvent::Device(DeviceEvent::WifiReconnected));
        let b = s.banner.as_ref().expect("banner present");
        assert!(matches!(b.kind, BannerKind::Success));
        assert!(b.duration.is_some(), "should be transient");
    }

    #[test]
    fn ipchanged_updates_active_device_ip() {
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(AppEvent::Device(DeviceEvent::Discovered(Device {
            serial: "1.2.3.4:5555".into(),
            name: "Pixel".into(),
            model: None,
            connection: fl_core::ConnectionKind::Wifi,
            state: fl_core::DeviceState::Online,
            ip: Some("1.2.3.4".into()),
            android_version: None,
            battery: None,
        })));
        s.apply(AppEvent::Device(DeviceEvent::IpChanged {
            serial: "1.2.3.4:5555".into(),
            old_ip: "1.2.3.4".into(),
            new_ip: "10.0.0.5".into(),
        }));
        assert_eq!(s.active_device.as_ref().unwrap().ip.as_deref(), Some("10.0.0.5"));
    }
}
