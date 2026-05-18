//! State mutated by `AppEvent`s and read by the renderer.

use fl_core::{AppEvent, DeviceEvent, FlutterEvent, LogLevel, VmEvent};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub fn short_name_for_serial(serial: &str) -> String {
    let mut s: String = serial.chars().filter(|c| c.is_alphanumeric()).take(8).collect();
    if s.is_empty() {
        s.push('?');
    }
    s
}

pub fn prefix_color_index(short_name: &str) -> usize {
    let mut hash: u64 = 5381;
    for b in short_name.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as u64);
    }
    (hash % 4) as usize
}

const LOG_RING: usize = 1_000;
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
    pub active_sessions: Vec<fl_core::DeviceSessionSummary>,
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
    pub started_at: Instant,
    /// Filled in when the first session reports AppStarted. Once set, the
    /// chronometer freezes at this duration instead of ticking live.
    pub compile_finished: Option<Duration>,
    /// If false, DEBUG log lines are hidden in the panel. Toggle with `v`.
    pub verbose: bool,
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
            active_sessions: Vec::new(),
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
            started_at: Instant::now(),
            compile_finished: None,
            verbose: false,
            quitting: false,
        }
    }

    /// Duration to display on the chronometer. Live until `compile_finished`
    /// is recorded, then frozen at that value.
    pub fn elapsed(&self) -> Duration {
        self.compile_finished.unwrap_or_else(|| self.started_at.elapsed())
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
                if let Some(sess) = self.active_sessions.iter_mut().find(|s| s.serial == d.serial) {
                    sess.state = fl_core::DeviceSessionState::Ready;
                    sess.ip = d.ip.clone();
                    sess.connection = d.connection;
                    sess.display_name = d.name.clone();
                    sess.platform = d.platform.clone();
                }
            }
            DeviceEvent::Lost { serial } => {
                if let Some(sess) = self.active_sessions.iter_mut().find(|s| s.serial == serial) {
                    sess.state = fl_core::DeviceSessionState::Stopped;
                }
            }
            DeviceEvent::UsbDisconnected { .. } => {
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
            DeviceEvent::IpChanged { new_ip, serial, .. } => {
                self.show_banner(BannerKind::Success, &format!("New IP: {new_ip}"));
                if let Some(sess) = self.active_sessions.iter_mut().find(|s| s.serial == serial) {
                    sess.ip = Some(new_ip.clone());
                }
            }
            DeviceEvent::SessionState { serial, state } => {
                if let Some(sess) = self.active_sessions.iter_mut().find(|s| s.serial == serial) {
                    sess.state = state;
                } else {
                    self.active_sessions.push(fl_core::DeviceSessionSummary {
                        serial: serial.clone(),
                        short_name: short_name_for_serial(&serial),
                        display_name: serial.clone(),
                        connection: if serial.contains(':') && serial.contains('.') {
                            fl_core::ConnectionKind::Wifi
                        } else {
                            fl_core::ConnectionKind::Usb
                        },
                        ip: None,
                        state,
                        platform: None,
                    });
                }
                // Auto-quit once every active session has reported Stopped.
                if !self.active_sessions.is_empty()
                    && self.active_sessions.iter().all(|s| {
                        matches!(s.state, fl_core::DeviceSessionState::Stopped
                                       | fl_core::DeviceSessionState::Failed)
                    })
                {
                    self.quitting = true;
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
                if self.compile_finished.is_none() {
                    self.compile_finished = Some(self.started_at.elapsed());
                    self.show_banner(BannerKind::Success, "App started — build done");
                }
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
                // Don't quit here — multi-device flow emits SessionState=Stopped
                // and apply_device decides when every session is done.
                self.push_log(LogLevel::Info, format!("flutter exited ({exit_code:?})"));
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

    pub(crate) fn expire_banner(&mut self) {
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

    /// Single place where every keypress is interpreted. Used by both the
    /// `View` impl and the legacy `TuiRunner::run` path. Returns the key
    /// untouched so the runner can also forward it to optional external
    /// handlers (e.g. a hot-reload dispatcher), without ever blocking.
    pub fn on_key(&mut self, key: fl_core::KeyEvent) {
        match key {
            fl_core::KeyEvent::Char('q') | fl_core::KeyEvent::Ctrl('c') => {
                self.quitting = true;
            }
            fl_core::KeyEvent::Char('v') => {
                self.verbose = !self.verbose;
                let msg = if self.verbose { "verbose: ON" } else { "verbose: OFF" };
                self.show_banner(BannerKind::Info, msg);
            }
            fl_core::KeyEvent::Char('r') => {
                self.flash_reload();
                self.show_banner(BannerKind::Info, "Hot reload requested (VM Service not yet wired)");
            }
            fl_core::KeyEvent::Char('R') => {
                self.show_banner(BannerKind::Info, "Hot restart requested (VM Service not yet wired)");
            }
            fl_core::KeyEvent::Char('c') => {
                self.logs.clear();
            }
            _ => {}
        }
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

impl crate::view::View for AppState {
    type Input = fl_core::AppEvent;

    fn apply(&mut self, input: Self::Input) {
        AppState::apply(self, input);
    }

    fn render(&self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer, theme: &crate::theme::Theme) {
        crate::render::render(area, buf, self, theme);
    }

    fn handle_key(&mut self, key: fl_core::KeyEvent) -> Option<Self::Input> {
        self.on_key(key);
        None
    }

    fn tick(&mut self, _dt: std::time::Duration) {
        self.expire_banner();
    }

    fn quitting(&self) -> bool {
        self.quitting
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use fl_core::{ConnectionKind, Device, DeviceState};

    #[test]
    fn session_state_event_creates_summary_for_unknown_serial() {
        let mut s = AppState::new("app".into(), "debug".into());
        s.apply(AppEvent::Device(DeviceEvent::SessionState {
            serial: "ABC".into(),
            state: fl_core::DeviceSessionState::Connecting,
        }));
        assert_eq!(s.active_sessions.len(), 1);
        assert_eq!(s.active_sessions[0].serial, "ABC");
        assert_eq!(s.active_sessions[0].state, fl_core::DeviceSessionState::Connecting);
    }

    #[test]
    fn discovered_marks_session_ready() {
        let mut s = AppState::new("app".into(), "debug".into());
        s.apply(AppEvent::Device(DeviceEvent::SessionState {
            serial: "ABC".into(),
            state: fl_core::DeviceSessionState::Connecting,
        }));
        s.apply(AppEvent::Device(DeviceEvent::Discovered(Device {
            serial: "ABC".into(),
            name: "Pixel".into(),
            model: None,
            connection: fl_core::ConnectionKind::Usb,
            state: fl_core::DeviceState::Online,
            ip: None,
            android_version: None,
            battery: None,
            platform: None,
        })));
        assert_eq!(s.active_sessions[0].state, fl_core::DeviceSessionState::Ready);
        assert_eq!(s.active_sessions[0].display_name, "Pixel");
    }

    #[test]
    fn ipchanged_updates_session_ip() {
        let mut s = AppState::new("app".into(), "debug".into());
        s.apply(AppEvent::Device(DeviceEvent::SessionState {
            serial: "1.2.3.4:5555".into(),
            state: fl_core::DeviceSessionState::Ready,
        }));
        s.apply(AppEvent::Device(DeviceEvent::IpChanged {
            serial: "1.2.3.4:5555".into(),
            old_ip: "1.2.3.4".into(),
            new_ip: "10.0.0.5".into(),
        }));
        assert_eq!(s.active_sessions[0].ip.as_deref(), Some("10.0.0.5"));
    }

    #[test]
    fn lost_marks_session_stopped() {
        let mut s = AppState::new("app".into(), "debug".into());
        s.apply(AppEvent::Device(DeviceEvent::SessionState {
            serial: "ABC".into(),
            state: fl_core::DeviceSessionState::Ready,
        }));
        s.apply(AppEvent::Device(DeviceEvent::Lost { serial: "ABC".into() }));
        assert_eq!(s.active_sessions[0].state, fl_core::DeviceSessionState::Stopped);
    }

    #[test]
    fn short_name_for_serial_truncates_to_8() {
        assert_eq!(short_name_for_serial("Pixel_8_AB12"), "Pixel8AB");
        assert_eq!(short_name_for_serial("192.168.1.42:5555"), "19216814");
        assert_eq!(short_name_for_serial(""), "?");
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

}
