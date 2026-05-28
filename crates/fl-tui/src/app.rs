//! State mutated by `AppEvent`s and read by the renderer.

use fl_core::{AppEvent, DeviceEvent, FlutterEvent, LogLevel, VmEvent};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub fn short_name_for_serial(serial: &str) -> String {
    let mut s: String = serial
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(8)
        .collect();
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
/// Maximum number of HTTP requests kept in the Network inspector
/// ring buffer. 200 is enough for the user to scroll back through a
/// typical session without exploding memory if the app is chatty.
const NETWORK_RING: usize = 200;
const MEM_RING: usize = 60;

#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: LogLevel,
    pub message: String,
}

/// Per-device performance state. One instance per active device serial
/// (see `AppState::device_perf`). Mirrors the global perf fields on
/// `AppState` but isolated, so the multi-device performance panel can
/// show each device's own FPS / memory trace.
#[derive(Debug, Default, Clone)]
pub struct DevicePerf {
    pub fps_samples: VecDeque<f32>,
    pub mem_samples: VecDeque<f32>,
    pub frame_ui_ms: f32,
    pub frame_raster_ms: f32,
    pub heap_capacity_mb: f32,
}

#[derive(Debug)]
pub struct AppState {
    pub app_name: String,
    pub mode: String,
    pub active_sessions: Vec<fl_core::DeviceSessionSummary>,
    pub logs: VecDeque<LogLine>,
    pub log_filter: Option<String>,
    /// `Some` while the user is typing the filter (between `/` and
    /// Enter / Esc). The buffer is mirrored into `log_filter` on every
    /// keystroke so the log view filters live as you type. Enter
    /// "freezes" the filter (sets this back to `None`, leaves the
    /// committed value in `log_filter`); Esc rewinds to whatever the
    /// filter was before `/` was pressed.
    pub filter_input: Option<String>,
    /// Snapshot of `log_filter` at the moment `/` was pressed, used by
    /// Esc to restore the previous state without making the user
    /// retype it.
    pub filter_saved: Option<String>,
    pub fps_samples: VecDeque<f32>,
    pub frame_ui_ms: f32,
    pub frame_raster_ms: f32,
    pub mem_samples: VecDeque<f32>,
    /// Last reported heap capacity in MB (from `getIsolateMemoryUsage`).
    /// Lets the panel show `used / capacity` instead of just `used`.
    pub heap_capacity_mb: f32,
    /// Per-device perf buffers, keyed by device serial. Populated by
    /// `AppEvent::Vm { serial, .. }` so the Performance panel can render
    /// one block of stats per running device. The global `fps_samples`,
    /// `mem_samples`, `frame_ui_ms`, etc. above remain as the "merged"
    /// view used by the single-device summary and by older tests.
    pub device_perf: std::collections::HashMap<String, DevicePerf>,
    /// Sliding window of Flutter.Frame event timestamps used to derive
    /// the *actual* frames-per-second the device is producing, separate
    /// from the per-frame `fps` computed off frame duration.
    pub frame_timestamps: VecDeque<Instant>,
    /// Monotonic count of Flutter.Frame events seen since boot. Survives
    /// the 240-entry trim window of `frame_timestamps` so the panel can
    /// show a stable "frames since start" total.
    pub total_frames: u64,
    pub rebuilds_per_sec: u32,
    pub vm_service_uri: Option<String>,
    pub vm_connected: bool,
    pub banner: Option<Banner>,
    pub last_reload_at: Option<Instant>,
    pub started_at: Instant,
    /// Filled in when the first session reports AppStarted. Once set, the
    /// chronometer freezes at this duration instead of ticking live.
    pub compile_finished: Option<Duration>,
    /// How many lines back from the tail the viewport's TOP currently sits.
    /// `0` means "follow tail" (live mode). Any other value freezes the view
    /// at that absolute window as new logs arrive (push_log increments
    /// this when > 0 to keep the user's anchor stable).
    ///
    /// Clamped at render time to `total.saturating_sub(viewport)` so scrolling
    /// past the oldest line is a no-op rather than emptying the viewport.
    pub log_scroll_offset: usize,
    /// Last viewport height observed by the renderer, in lines. Read by the
    /// key handler so PageUp/PageDown move a real screenful and Up stops at
    /// the oldest line instead of letting the viewport collapse.
    pub log_viewport_height: std::sync::atomic::AtomicUsize,
    /// Shared with the key dispatcher. Values: 0 = system (no override),
    /// 1 = light, 2 = dark. Cycled by `b` matching `flutter run` semantics
    /// so the user gets a guaranteed visual change on every press even if
    /// their OS is already in dark mode.
    pub brightness_state: std::sync::Arc<std::sync::atomic::AtomicU8>,
    /// Whether Flutter's `debugPaintSize` overlay is currently enabled.
    /// Toggled by `p`; the key dispatcher reads this and forwards the
    /// new value to the VM Service so a second press actually clears
    /// the overlay instead of stacking another "enable" call.
    pub debug_paint_on: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Current platform override: `false` = android, `true` = iOS.
    /// Toggled by `o`. Like `debug_paint_on`, the dispatcher reads this
    /// so each press flips the device's reported platform instead of
    /// always re-asserting the same value.
    pub platform_is_ios: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Whether Flutter's on-device performance overlay (the FPS strip
    /// drawn on top of the running app via `showPerformanceOverlay`) is
    /// currently visible. Toggled by `P`.
    pub perf_overlay_on: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// When true, the left dashboard panel renders the Network
    /// inspector (live HTTP requests) instead of the Performance
    /// panel. Toggled by `n`.
    pub show_network: bool,
    /// Rolling buffer of recent HTTP requests captured from the
    /// running app's Dart VM (`ext.dart.io.getHttpProfile`). Latest
    /// at the back. Capped at NETWORK_RING entries.
    pub network_requests: VecDeque<NetworkRequest>,
    /// How many lines back from the *latest* request the Network
    /// panel's viewport is anchored. `0` = follow the tail (newest
    /// request always at the bottom). Up arrow increments, Down
    /// decrements, clamped at render time to `len - viewport_height`
    /// so scrolling stops at the oldest visible row instead of
    /// pushing requests off the top.
    pub network_scroll_offset: usize,
    /// Last viewport height observed by the network renderer, in
    /// rows. Read by the Up handler to cap `network_scroll_offset`
    /// at `len - height`. AtomicUsize so the renderer can write to
    /// it without making `render` take `&mut`.
    pub network_viewport_height: std::sync::atomic::AtomicUsize,
    pub quitting: bool,
    /// Absolute path of the Flutter project root (= cwd at startup).
    /// Used by the `e` keybind to turn a relative `lib/foo.dart` path into
    /// the absolute path the IDE needs.
    pub project_root: std::path::PathBuf,
    /// Lazily-detected IDE. `None` = not yet probed; `Some(None)` = probed
    /// but nothing found; `Some(Some(kind))` = found.
    pub ide_cache: Option<Option<crate::ide::IdeKind>>,
    /// All `app.progress` phases observed so far in the current run,
    /// in arrival order. The last entry's `finished_at == None` ⇒ it's
    /// the *active* phase (what the user is waiting on right now).
    /// Drives the loading bar shown in the dashboard header until the
    /// VM Service is connected.
    pub progress_phases: Vec<ProgressPhase>,
}

/// A single Flutter daemon progress phase — e.g. "Running Xcode
/// build...", "Installing and launching...". We track the wall-clock
/// span (`started_at` → `finished_at`) so the UI can both colour the
/// stepper (done ✓ vs current ⏳) and show per-phase elapsed time.
#[derive(Debug, Clone)]
pub struct ProgressPhase {
    /// Per-event daemon id (e.g. `"3"`). Used to match the closing
    /// `finished: true` event back to the opener, since the daemon
    /// reuses the same id for both.
    pub id: String,
    /// Phase tag the daemon attached (`devFS.update`, `hot.reload`, …).
    /// `None` when the daemon didn't tag the event — many startup
    /// phases are untagged and we pivot on `message` instead.
    pub progress_id: Option<String>,
    /// Human-readable phase title (`"Running Xcode build..."`, …).
    pub message: String,
    /// Local time the phase started.
    pub started_at: std::time::Instant,
    /// Local time the phase finished, `None` while still in progress.
    pub finished_at: Option<std::time::Instant>,
}

/// Single HTTP request snapshot, captured by polling
/// `ext.dart.io.getHttpProfile` on each session's VM Service.
#[derive(Debug, Clone)]
pub struct NetworkRequest {
    /// Originating device's short_name — so a single Network panel
    /// can show traffic from N devices without mixing them up.
    pub device: String,
    /// HTTP method (GET/POST/…)
    pub method: String,
    /// Request URL (full, untruncated; rendering truncates).
    pub url: String,
    /// Response status code, `None` while still in flight or on error.
    pub status: Option<u16>,
    /// Duration in ms from request start to response end. `None`
    /// while in flight.
    pub duration_ms: Option<u64>,
    /// Network-level error message (e.g. "Connection refused"). When
    /// `Some`, the row is rendered in red regardless of `status`.
    pub error: Option<String>,
}

/// Possible values for `AppState::brightness_state`.
pub const BRIGHTNESS_SYSTEM: u8 = 0;
pub const BRIGHTNESS_LIGHT: u8 = 1;
pub const BRIGHTNESS_DARK: u8 = 2;

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
            filter_input: None,
            filter_saved: None,
            fps_samples: VecDeque::with_capacity(FPS_RING),
            frame_ui_ms: 0.0,
            frame_raster_ms: 0.0,
            mem_samples: VecDeque::with_capacity(MEM_RING),
            heap_capacity_mb: 0.0,
            device_perf: std::collections::HashMap::new(),
            frame_timestamps: VecDeque::with_capacity(240),
            total_frames: 0,
            rebuilds_per_sec: 0,
            vm_service_uri: None,
            vm_connected: false,
            banner: None,
            last_reload_at: None,
            started_at: Instant::now(),
            compile_finished: None,
            log_scroll_offset: 0,
            log_viewport_height: std::sync::atomic::AtomicUsize::new(20),
            brightness_state: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                BRIGHTNESS_SYSTEM,
            )),
            debug_paint_on: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            platform_is_ios: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            perf_overlay_on: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            show_network: false,
            network_requests: VecDeque::with_capacity(NETWORK_RING),
            network_scroll_offset: 0,
            network_viewport_height: std::sync::atomic::AtomicUsize::new(10),
            quitting: false,
            project_root: std::env::current_dir().unwrap_or_default(),
            ide_cache: None,
            progress_phases: Vec::new(),
        }
    }

    /// Clone-able handle to the debug-paint flag, so the multi-device key
    /// dispatcher in flutter-cli can read the same value the TUI mutates.
    pub fn debug_paint_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.debug_paint_on.clone()
    }

    /// Clone-able handle to the platform-override flag.
    pub fn platform_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.platform_is_ios.clone()
    }

    /// Clone-able handle to the on-device performance overlay flag.
    pub fn perf_overlay_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.perf_overlay_on.clone()
    }

    /// Clone-able handle to the brightness state, so the multi-device key
    /// dispatcher in flutter-cli can read the same value the TUI displays.
    pub fn brightness_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicU8> {
        self.brightness_state.clone()
    }

    /// True once the app is fully running and the VM Service is live —
    /// the moment when r/R/b/p/o/P all actually do something. Used by
    /// the renderer to hide those keys from the footer while we're
    /// still compiling, so the user isn't tempted to press them and
    /// spam the logs with "not ready" warnings.
    pub fn app_ready(&self) -> bool {
        self.vm_connected
    }

    /// The currently-active progress phase, if any. `None` once every
    /// recorded phase has finished — used by the renderer to decide
    /// whether to show the loading strip below the header.
    pub fn current_progress_phase(&self) -> Option<&ProgressPhase> {
        self.progress_phases
            .iter()
            .rev()
            .find(|p| p.finished_at.is_none())
    }

    /// Duration to display on the chronometer. Live until `compile_finished`
    /// is recorded, then frozen at that value.
    pub fn elapsed(&self) -> Duration {
        self.compile_finished
            .unwrap_or_else(|| self.started_at.elapsed())
    }

    pub fn apply(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Device(d) => self.apply_device(d),
            AppEvent::Flutter(f) => self.apply_flutter(f),
            AppEvent::Vm { serial, event } => self.apply_vm(&serial, event),
            AppEvent::Key(_) | AppEvent::Tick => {}
        }
        self.expire_banner();
    }

    fn apply_device(&mut self, ev: DeviceEvent) {
        match ev {
            DeviceEvent::Discovered(d) => {
                if let Some(sess) = self
                    .active_sessions
                    .iter_mut()
                    .find(|s| s.serial == d.serial)
                {
                    // "Discovered" means adb / devicectl sees the
                    // device physically attached — NOT that the
                    // Flutter app has started on it. Only flutter-cli's
                    // `AppStarted` handler in multi.rs sends
                    // `SessionState::Ready`, which is the real
                    // signal we want. Leaving state alone here
                    // prevents the dashboard from prematurely
                    // marking an iPhone as Ready (and freezing the
                    // chronometer on ✓) while its Xcode build is
                    // still in progress. We still pick up metadata
                    // (IP, connection kind, name, platform) — those
                    // are independent of whether the app is live.
                    sess.ip = d.ip.clone();
                    sess.connection = d.connection;
                    // The `host:track-devices` payload only carries the
                    // serial — `track_devices` therefore emits Discovered
                    // events with `name == serial`. If we let that
                    // overwrite a properly-resolved display name (e.g.
                    // "SM A145R" from `getprop ro.product.model`), the
                    // panel falls back to showing the raw serial. Only
                    // accept a name that actually adds information.
                    if d.name != d.serial && !d.name.is_empty() {
                        sess.display_name = d.name.clone();
                    }
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
                let prev = self
                    .active_sessions
                    .iter()
                    .find(|s| s.serial == serial)
                    .map(|s| s.state);
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
                // Surface the transition into Stopped/Failed so the user
                // knows why their app went away — but do NOT auto-quit the
                // TUI. They unplugged the cable on purpose and still want
                // to read the logs / scroll back. `q` is always an option.
                let became_terminal = matches!(
                    state,
                    fl_core::DeviceSessionState::Stopped | fl_core::DeviceSessionState::Failed
                ) && !matches!(
                    prev,
                    Some(fl_core::DeviceSessionState::Stopped)
                        | Some(fl_core::DeviceSessionState::Failed)
                );
                if became_terminal {
                    let short = short_name_for_serial(&serial);
                    self.show_persistent_banner(
                        BannerKind::Warn,
                        &format!("[{short}] device disconnected — press q to exit"),
                    );
                }
                // Freeze the chronometer on green (✓) only when EVERY
                // active session has reached `Ready`. On a single-
                // device run that's the moment the first AppStarted
                // fires; on multi-device it waits for the slowest
                // device's build to finish. Without this guard the
                // header would show ✓ as soon as the fastest device
                // booted while iPhone (slower) was still compiling.
                let all_ready = !self.active_sessions.is_empty()
                    && self
                        .active_sessions
                        .iter()
                        .all(|s| matches!(s.state, fl_core::DeviceSessionState::Ready));
                if all_ready && self.compile_finished.is_none() {
                    self.compile_finished = Some(self.started_at.elapsed());
                    let label = if self.active_sessions.len() == 1 {
                        "App started — build done"
                    } else {
                        "All devices compiled and launched"
                    };
                    self.show_banner(BannerKind::Success, label);
                }
            }
            DeviceEvent::HttpRequest {
                device,
                method,
                url,
                status,
                duration_ms,
                error,
            } => {
                // Append to the rolling buffer the Network panel
                // reads from. Cap at NETWORK_RING to keep memory
                // bounded — old requests fall off the front.
                self.network_requests.push_back(NetworkRequest {
                    device,
                    method,
                    url,
                    status,
                    duration_ms,
                    error,
                });
                while self.network_requests.len() > NETWORK_RING {
                    self.network_requests.pop_front();
                    // The user's scroll anchor is "N back from
                    // newest", but when an old entry drops off the
                    // front the *content* under that anchor shifts.
                    // Decrement so what the user is looking at stays
                    // in place.
                    if self.network_scroll_offset > 0 {
                        self.network_scroll_offset -= 1;
                    }
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
                // The chronometer freezing on green (✓) is now
                // driven by `apply_device(SessionState=Ready)` once
                // *every* active session has reported Ready —
                // otherwise on a multi-device run the timer would
                // turn green the moment the first device finished,
                // even though the others were still compiling.
            }
            FlutterEvent::Log { level, message } => self.push_log(level, message),
            FlutterEvent::Progress {
                id,
                progress_id,
                message,
                finished,
            } => {
                // Track the phase for the dashboard loading bar. The
                // daemon emits TWO events per phase: an opener (with
                // message + finished=false) and a closer (finished=true,
                // usually with no message). We match them up by `id`.
                if finished {
                    if let Some(p) = self
                        .progress_phases
                        .iter_mut()
                        .rev()
                        .find(|p| p.id == id && p.finished_at.is_none())
                    {
                        p.finished_at = Some(std::time::Instant::now());
                    }
                } else if !message.is_empty() {
                    self.progress_phases.push(ProgressPhase {
                        id,
                        progress_id,
                        message: message.clone(),
                        started_at: std::time::Instant::now(),
                        finished_at: None,
                    });
                }
                // Mirror to logs so the user can still scroll back to
                // every phase in scrollback — same UX as before.
                if !message.is_empty() {
                    self.push_log(
                        if finished {
                            LogLevel::Info
                        } else {
                            LogLevel::Debug
                        },
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

    fn apply_vm(&mut self, serial: &str, ev: VmEvent) {
        match ev {
            VmEvent::Connected => self.vm_connected = true,
            VmEvent::Disconnected => self.vm_connected = false,
            VmEvent::Stdout(s) => self.push_log(LogLevel::Info, s.trim_end().into()),
            VmEvent::Stderr(s) => self.push_log(LogLevel::Error, s.trim_end().into()),
            VmEvent::FrameTiming {
                ui_micros,
                raster_micros,
            } => {
                let total_ms = (ui_micros + raster_micros) as f32 / 1000.0;
                let fps = if total_ms > 0.0 {
                    1000.0 / total_ms
                } else {
                    0.0
                };
                push_capped(&mut self.fps_samples, fps.clamp(0.0, 120.0), FPS_RING);
                self.frame_ui_ms = ui_micros as f32 / 1000.0;
                self.frame_raster_ms = raster_micros as f32 / 1000.0;
                // Per-device sample so the multi-device panel can show
                // one FPS sparkline per running device. Skip when the
                // serial is empty (legacy / test events).
                if !serial.is_empty() {
                    let perf = self.device_perf.entry(serial.to_string()).or_default();
                    push_capped(&mut perf.fps_samples, fps.clamp(0.0, 120.0), FPS_RING);
                    perf.frame_ui_ms = ui_micros as f32 / 1000.0;
                    perf.frame_raster_ms = raster_micros as f32 / 1000.0;
                }
                // Record event arrival for actual frames/s. Trim anything
                // older than 2 s so the deque stays bounded even at 120 Hz.
                self.total_frames = self.total_frames.saturating_add(1);
                let now = Instant::now();
                self.frame_timestamps.push_back(now);
                while let Some(front) = self.frame_timestamps.front() {
                    if now.duration_since(*front) > Duration::from_secs(2) {
                        self.frame_timestamps.pop_front();
                    } else {
                        break;
                    }
                }
            }
            VmEvent::GcStats { used_mb, total_mb } => {
                push_capped(&mut self.mem_samples, used_mb as f32, MEM_RING);
                self.heap_capacity_mb = total_mb as f32;
                if !serial.is_empty() {
                    let perf = self.device_perf.entry(serial.to_string()).or_default();
                    push_capped(&mut perf.mem_samples, used_mb as f32, MEM_RING);
                    perf.heap_capacity_mb = total_mb as f32;
                }
            }
            VmEvent::IsolateEvent(_) | VmEvent::ExtensionResult { .. } => {}
        }
    }

    pub fn push_log(&mut self, level: LogLevel, message: String) {
        // Sanitise the message before storing it: strip ANSI escape
        // sequences (xcodebuild loves emitting `\x1b[31m…\x1b[0m`),
        // replace embedded `\n`/`\r`/`\t` with spaces, and cap the
        // length. Without this, a single rogue daemon line can corrupt
        // the entire dashboard rendering (terminal control chars get
        // re-interpreted) and force a Ctrl-L redraw.
        let message = sanitize_log_message(message);

        if self.logs.len() >= LOG_RING {
            self.logs.pop_front();
            // Ring popped the oldest entry. If the user was scrolled up,
            // their anchor in absolute terms hasn't moved — but the indices
            // shifted by 1 to the left, so offset effectively stays the same.
            // (See note below: push then bumps it.)
        }
        self.logs.push_back(LogLine { level, message });
        // The user is scrolled into history → bump offset to keep the
        // exact same lines on screen as new logs arrive at the tail.
        if self.log_scroll_offset > 0 {
            let max = self.max_log_scroll_offset();
            self.log_scroll_offset = (self.log_scroll_offset + 1).min(max);
        }
    }

    /// Visible viewport height used by the log panel — fed back into scroll
    /// math so PageUp/PageDown move a real screenful and Up stops at the
    /// oldest line. Set by the renderer each frame.
    pub fn log_viewport_lines(&self) -> usize {
        self.log_viewport_height
            .load(std::sync::atomic::Ordering::Relaxed)
            .max(1)
    }

    /// Highest meaningful scroll offset given the current log count and the
    /// last-observed viewport height. At this offset the very oldest line
    /// sits at the top of the panel and Up becomes a no-op.
    pub fn max_log_scroll_offset(&self) -> usize {
        self.logs.len().saturating_sub(self.log_viewport_lines())
    }

    /// Find the most-recent user-space Dart file reference in the log ring
    /// and open it in the detected IDE. Returns a short status string for
    /// the banner.
    pub fn open_in_ide(&mut self) -> String {
        // Lazy IDE detection — probe once, cache the result.
        let ide = match self.ide_cache {
            Some(cached) => cached,
            None => {
                let detected = crate::ide::detect();
                self.ide_cache = Some(detected);
                detected
            }
        };

        let Some(ide_kind) = ide else {
            return "No IDE detected — open VS Code or Android Studio first".to_string();
        };

        // Scan from newest (back of deque) to oldest looking for a file ref.
        let file_ref = self.logs.iter().rev().find_map(|l| {
            crate::ide::extract_file_ref(&l.message)
        });

        match file_ref {
            None => "No Dart file reference found in logs".to_string(),
            Some(r) => crate::ide::open(ide_kind, &self.project_root, &r),
        }
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
        // Filter-input mode: keystrokes build up the filter string.
        // We check `filter_input` (the active-typing buffer) rather
        // than `log_filter` (the committed filter) so keys like `c`
        // pressed *after* the filter is applied are dispatched to
        // their normal handler instead of being appended to the
        // already-committed pattern.
        if self.filter_input.is_some() {
            self.handle_filter_key(key);
            return;
        }
        match key {
            fl_core::KeyEvent::Char('q') | fl_core::KeyEvent::Ctrl('c') => {
                self.quitting = true;
            }
            fl_core::KeyEvent::Up => {
                // While the Network panel is visible, Up/Down scroll
                // its history instead of the log scrollback — that's
                // where the user's eyes are looking. The max offset
                // is `len - viewport_height` so the oldest request
                // stays pinned to the top instead of getting pushed
                // off when the user keeps pressing Up.
                if self.show_network {
                    let viewport = self
                        .network_viewport_height
                        .load(std::sync::atomic::Ordering::Relaxed)
                        .max(1);
                    let max = self.network_requests.len().saturating_sub(viewport);
                    self.network_scroll_offset = (self.network_scroll_offset + 1).min(max);
                } else {
                    let max = self.max_log_scroll_offset();
                    self.log_scroll_offset = (self.log_scroll_offset + 1).min(max);
                }
            }
            fl_core::KeyEvent::Down => {
                if self.show_network {
                    self.network_scroll_offset = self.network_scroll_offset.saturating_sub(1);
                } else {
                    self.log_scroll_offset = self.log_scroll_offset.saturating_sub(1);
                }
            }
            fl_core::KeyEvent::PageUp => {
                let max = self.max_log_scroll_offset();
                let step = self.log_viewport_lines();
                self.log_scroll_offset = (self.log_scroll_offset + step).min(max);
            }
            fl_core::KeyEvent::PageDown => {
                let step = self.log_viewport_lines();
                self.log_scroll_offset = self.log_scroll_offset.saturating_sub(step);
            }
            fl_core::KeyEvent::Char('g') => {
                // Jump back to live tail (follow newest).
                self.log_scroll_offset = 0;
            }
            fl_core::KeyEvent::Char('G') => {
                // Jump to the oldest entry currently retained — at this
                // offset the very first line sits at the top of the panel.
                self.log_scroll_offset = self.max_log_scroll_offset();
            }
            fl_core::KeyEvent::Char('r') => {
                self.flash_reload();
                self.show_banner(BannerKind::Info, "Hot reload");
            }
            fl_core::KeyEvent::Char('R') => {
                self.flash_reload();
                self.show_banner(BannerKind::Info, "Hot restart");
            }
            fl_core::KeyEvent::Char('b') => {
                use std::sync::atomic::Ordering;
                // Cycle system → light → dark → system (matches `flutter run`).
                let cur = self.brightness_state.load(Ordering::Relaxed);
                let next = match cur {
                    BRIGHTNESS_SYSTEM => BRIGHTNESS_LIGHT,
                    BRIGHTNESS_LIGHT => BRIGHTNESS_DARK,
                    _ => BRIGHTNESS_SYSTEM,
                };
                self.brightness_state.store(next, Ordering::Relaxed);
                let label = match next {
                    BRIGHTNESS_LIGHT => "☀️ Light",
                    BRIGHTNESS_DARK => "🌙 Dark",
                    _ => "⚙️ Settings",
                };
                self.show_banner(BannerKind::Info, &format!("Brightness → {label}"));
            }
            fl_core::KeyEvent::Char('p') => {
                use std::sync::atomic::Ordering;
                let next = !self.debug_paint_on.load(Ordering::Relaxed);
                self.debug_paint_on.store(next, Ordering::Relaxed);
                let label = if next {
                    "Debug paint: ON"
                } else {
                    "Debug paint: OFF"
                };
                self.show_banner(BannerKind::Info, label);
            }
            fl_core::KeyEvent::Char('o') => {
                use std::sync::atomic::Ordering;
                let next = !self.platform_is_ios.load(Ordering::Relaxed);
                self.platform_is_ios.store(next, Ordering::Relaxed);
                let label = if next {
                    "Platform → iOS"
                } else {
                    "Platform → Android"
                };
                self.show_banner(BannerKind::Info, label);
            }
            fl_core::KeyEvent::Char('P') => {
                use std::sync::atomic::Ordering;
                let next = !self.perf_overlay_on.load(Ordering::Relaxed);
                self.perf_overlay_on.store(next, Ordering::Relaxed);
                let label = if next {
                    "Perf overlay: ON (on-device FPS strip)"
                } else {
                    "Perf overlay: OFF"
                };
                self.show_banner(BannerKind::Info, label);
            }
            fl_core::KeyEvent::Char('e') => {
                // Open the first user-space Dart file reference found in
                // the visible log window in the detected IDE (VS Code or
                // Android Studio). Scans from newest to oldest so the most
                // recent error is always the jump target.
                let msg = self.open_in_ide();
                let kind = if msg.contains("VS Code")
                    || msg.contains("Android Studio")
                    || msg.contains("Opened")
                {
                    BannerKind::Success
                } else {
                    BannerKind::Warn
                };
                self.show_banner(kind, &msg);
            }
            fl_core::KeyEvent::Char('n') => {
                // Toggle the left dashboard panel between Performance
                // and Network inspector. Purely local state — the
                // network data continues to be collected in the
                // background whether or not the panel is visible.
                self.show_network = !self.show_network;
                self.show_banner(
                    BannerKind::Info,
                    if self.show_network {
                        "🌐 Network panel"
                    } else {
                        "📊 Performance panel"
                    },
                );
            }
            fl_core::KeyEvent::Char('c') => {
                // Copy the visible log contents to the clipboard, stripped
                // of level prefixes (INFO/DEBUG/…) and the per-device
                // short-name prefix (e.g. `[00008140]`). When a `/` filter
                // is active, we copy ONLY the matching lines — same set
                // the user sees on screen — so "filter then copy" works
                // exactly as expected for triaging a noisy log.
                let filter = self.log_filter.as_deref();
                let matching: Vec<&LogLine> = self
                    .logs
                    .iter()
                    .filter(|l| crate::runner::log_matches_filter(filter, l.level, &l.message))
                    .collect();
                let text = matching
                    .iter()
                    .map(|l| strip_log_prefix(&l.message))
                    .collect::<Vec<_>>()
                    .join("\n");
                let count = matching.len();
                match copy_to_clipboard(&text) {
                    Ok(()) => {
                        let label = match filter {
                            Some(f) if !f.is_empty() => format!("📋 Copied {count} · `{f}`"),
                            _ => format!("📋 Copied {count}"),
                        };
                        self.show_banner(BannerKind::Success, &label);
                    }
                    Err(e) => self.show_banner(BannerKind::Error, &format!("Copy failed: {e}")),
                }
            }
            fl_core::KeyEvent::Char('/') => {
                // Snapshot the active filter so Esc can roll back to
                // it cleanly. Seed the input buffer with that value so
                // the user can edit instead of retyping from scratch.
                self.filter_saved = self.log_filter.clone();
                self.filter_input = Some(self.log_filter.clone().unwrap_or_default());
                let preview = self.filter_input.clone().unwrap_or_default();
                self.show_persistent_banner(BannerKind::Info, &format!("Filter: {preview}"));
            }
            _ => {}
        }
    }

    fn handle_filter_key(&mut self, key: fl_core::KeyEvent) {
        match key {
            fl_core::KeyEvent::Esc => {
                // Cancel: drop the in-progress buffer AND restore the
                // filter that was active right before `/` was pressed.
                self.filter_input = None;
                self.log_filter = self.filter_saved.take();
                self.clear_persistent_banner();
                self.show_banner(BannerKind::Info, "Filter cancelled");
            }
            fl_core::KeyEvent::Enter => {
                // Freeze: exit input mode, keep whatever's already in
                // `log_filter` (the live-typed value). No further work
                // needed because every keystroke below already mirrors
                // the buffer into `log_filter`.
                self.filter_input = None;
                self.filter_saved = None;
                self.clear_persistent_banner();
                let label = match self.log_filter.as_deref() {
                    Some(f) if !f.is_empty() => format!("Filter: {f}"),
                    _ => "Filter cleared".to_string(),
                };
                self.show_banner(BannerKind::Success, &label);
            }
            fl_core::KeyEvent::Backspace => {
                if let Some(f) = self.filter_input.as_mut() {
                    f.pop();
                    let f_clone = f.clone();
                    self.log_filter = if f_clone.is_empty() {
                        None
                    } else {
                        Some(f_clone.clone())
                    };
                    self.show_persistent_banner(BannerKind::Info, &format!("Filter: {f_clone}"));
                }
            }
            fl_core::KeyEvent::Char(c) => {
                if let Some(f) = self.filter_input.as_mut() {
                    f.push(c);
                    let f_clone = f.clone();
                    // Mirror into the active filter on every keystroke
                    // so the log view re-filters live as the user types.
                    self.log_filter = Some(f_clone.clone());
                    self.show_persistent_banner(BannerKind::Info, &format!("Filter: {f_clone}"));
                }
            }
            _ => {}
        }
    }

    /// Actual frames-per-second observed from `Flutter.Frame` event arrival
    /// rate over the last second. Differs from the per-frame `fps` (which is
    /// `1000/frame_ms` for the most recent frame) — this is the rate Flutter
    /// is *actually* shipping frames at.
    pub fn frames_per_sec(&self) -> f32 {
        let now = Instant::now();
        let window = Duration::from_secs(1);
        self.frame_timestamps
            .iter()
            .filter(|t| now.duration_since(**t) <= window)
            .count() as f32
    }

    /// Fraction of the most-recent FPS samples that fell below 55 FPS — a
    /// rough proxy for jank.
    pub fn jank_ratio(&self) -> f32 {
        let n = self.fps_samples.len();
        if n == 0 {
            return 0.0;
        }
        let janky = self.fps_samples.iter().filter(|f| **f < 55.0).count();
        janky as f32 / n as f32
    }

    pub fn reload_flash_alpha(&self) -> f32 {
        match self.last_reload_at {
            Some(t) => {
                let ms = t.elapsed().as_millis() as f32;
                // 1500 ms so the "Refresh…" shimmer in the header has time
                // to actually play through one sweep before fading out.
                (1.0 - ms / 1500.0).clamp(0.0, 1.0)
            }
            None => 0.0,
        }
    }
}

/// Hard cap on stored log line length. Anything beyond is silently
/// truncated with an `…` marker. Keeps the ring buffer bounded even
/// when a daemon emits multi-MB JSON dumps (devicectl in `-v` mode is
/// the usual suspect).
const MAX_LOG_LINE_CHARS: usize = 800;

/// Strip ANSI escape sequences and replace embedded control characters
/// with a single space so a single rogue daemon line can't corrupt the
/// terminal display. Also truncates to `MAX_LOG_LINE_CHARS` to keep
/// the ring buffer bounded.
///
/// We allocate at most one new String — and skip even that when the
/// input already happens to be clean (the common case for Dart
/// `print` output).
fn sanitize_log_message(input: String) -> String {
    // Any C0 control byte (0x00–0x1F) — ESC, newlines, tabs, BEL, NUL,
    // etc. — disqualifies the input for the fast path. They all need
    // sanitisation either to strip a control sequence or to convert
    // the byte to a visible space.
    let needs_work = input.bytes().any(|b| b < 0x20) || input.chars().count() > MAX_LOG_LINE_CHARS;
    if !needs_work {
        return input;
    }
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut written: usize = 0;
    while let Some(c) = chars.next() {
        if written >= MAX_LOG_LINE_CHARS {
            out.push('…');
            break;
        }
        // ANSI CSI: ESC `[` <params> <final>. Final byte is in 0x40..=0x7E.
        // We also catch the much simpler ESC + single-char sequences.
        if c == '\u{001B}' {
            if matches!(chars.peek(), Some('[')) {
                chars.next();
                for cc in chars.by_ref() {
                    if matches!(cc, '\u{40}'..='\u{7E}') {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        // Replace embedded line-breaks and tabs with a single space so
        // the log stays on its own row in the ratatui Paragraph.
        if matches!(c, '\n' | '\r' | '\t') {
            // Collapse consecutive whitespace produced by this pass.
            if !out.ends_with(' ') {
                out.push(' ');
                written += 1;
            }
            continue;
        }
        // Drop the rest of the C0 control range (NUL, BEL, etc.).
        if (c as u32) < 0x20 {
            continue;
        }
        out.push(c);
        written += 1;
    }
    out
}

/// Strip the `[XXXXXXXX]` device short-name prefix that we add to each
/// daemon log line for multi-device sessions. Leading/trailing
/// whitespace is also trimmed so paste-ready text stays clean.
///
/// Examples:
///   `"[00008140] Reloaded 1 of 2449 libraries in 313ms…"` → `"Reloaded 1 of 2449 libraries in 313ms…"`
///   `"daemon ready"` (no prefix) → `"daemon ready"`
fn strip_log_prefix(msg: &str) -> String {
    let trimmed = msg.trim_start();
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some(close) = rest.find(']') {
            let inside = &rest[..close];
            // Only strip if it looks like a serial / hex tag, not a
            // bracketed regular word the user might want preserved.
            if inside
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == '-' || c == '_')
            {
                return rest[close + 1..].trim().to_string();
            }
        }
    }
    trimmed.to_string()
}

/// Copy `text` to the OS clipboard. Uses `arboard` so the same code
/// works on macOS (NSPasteboard), Linux (X11 / Wayland) and Windows
/// (clipboard API) — the previous `Command::new("pbcopy")` path only
/// worked on macOS and bombed elsewhere with a cryptic ENOENT.
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| std::io::Error::other(format!("clipboard unavailable: {e}")))?;
    clipboard
        .set_text(text)
        .map_err(|e| std::io::Error::other(format!("clipboard write failed: {e}")))?;
    Ok(())
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

    fn render(
        &self,
        area: ratatui::layout::Rect,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
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
        assert_eq!(
            s.active_sessions[0].state,
            fl_core::DeviceSessionState::Connecting
        );
    }

    #[test]
    fn discovered_keeps_state_but_updates_metadata() {
        // Discovered means the device is physically connected — it
        // does NOT mean the Flutter app has started on it. The
        // session state must NOT be promoted to Ready by a
        // Discovered event; only `AppStarted` (which flutter-cli
        // translates into `SessionState::Ready`) should do that.
        // Metadata fields (display_name, IP, platform, connection)
        // are however allowed to update freely.
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
        assert_eq!(
            s.active_sessions[0].state,
            fl_core::DeviceSessionState::Connecting,
            "Discovered must not promote a Connecting session to Ready"
        );
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
        s.apply(AppEvent::Device(DeviceEvent::Lost {
            serial: "ABC".into(),
        }));
        assert_eq!(
            s.active_sessions[0].state,
            fl_core::DeviceSessionState::Stopped
        );
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
        s.apply(AppEvent::Vm {
            serial: String::new(),
            event: VmEvent::FrameTiming {
                ui_micros: 5_000,
                raster_micros: 11_000,
            },
        });
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
        s.apply(AppEvent::Device(DeviceEvent::UsbDisconnected {
            serial: "X".into(),
        }));
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
        assert!(
            s.banner.is_some(),
            "transient banner should survive clear_persistent_banner"
        );

        s.show_persistent_banner(BannerKind::Warn, "sticky");
        s.clear_persistent_banner();
        assert!(s.banner.is_none(), "persistent banner should be cleared");
    }

    #[test]
    fn wifi_reconnecting_sets_persistent_warn_banner() {
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(AppEvent::Device(DeviceEvent::WifiReconnecting {
            attempt: 3,
        }));
        let b = s.banner.as_ref().expect("banner present");
        assert!(matches!(b.kind, BannerKind::Warn));
        assert!(b.duration.is_none(), "should be persistent");
        assert!(b.message.contains("#3"));
    }

    #[test]
    fn wifi_reconnected_clears_persistent_and_shows_success() {
        let mut s = AppState::new("a".into(), "d".into());
        s.apply(AppEvent::Device(DeviceEvent::WifiReconnecting {
            attempt: 1,
        }));
        s.apply(AppEvent::Device(DeviceEvent::WifiReconnected));
        let b = s.banner.as_ref().expect("banner present");
        assert!(matches!(b.kind, BannerKind::Success));
        assert!(b.duration.is_some(), "should be transient");
    }

    #[test]
    fn sanitize_strips_ansi_color_codes() {
        let raw = "before\u{001B}[31mRED\u{001B}[0m after".to_string();
        assert_eq!(sanitize_log_message(raw), "beforeRED after");
    }

    #[test]
    fn sanitize_replaces_newlines_with_space() {
        let raw = "line one\nline two\r\nline three".to_string();
        // Consecutive line-breaks collapse to a single space.
        assert_eq!(sanitize_log_message(raw), "line one line two line three");
    }

    #[test]
    fn sanitize_truncates_extremely_long_lines() {
        let raw = "a".repeat(2_000);
        let out = sanitize_log_message(raw);
        let chars = out.chars().count();
        assert!(chars <= MAX_LOG_LINE_CHARS + 1, "got {chars} chars");
        assert!(out.ends_with('…'));
    }

    #[test]
    fn sanitize_keeps_clean_input_untouched_and_does_not_realloc() {
        // No control chars, under the cap → returned as-is. We can't
        // observe "no realloc" directly, but we can at least verify
        // the value is unchanged.
        let clean = "Reloaded 1 of 2449 libraries in 313ms".to_string();
        let original_ptr = clean.as_ptr();
        let out = sanitize_log_message(clean);
        assert_eq!(out, "Reloaded 1 of 2449 libraries in 313ms");
        assert_eq!(out.as_ptr(), original_ptr, "should not reallocate");
    }

    #[test]
    fn strip_log_prefix_removes_device_serial_tag() {
        assert_eq!(
            strip_log_prefix("[00008140] Reloaded 1 of 2449 libraries in 313ms"),
            "Reloaded 1 of 2449 libraries in 313ms"
        );
        assert_eq!(strip_log_prefix("[ABC-DEF_123] hot reload"), "hot reload");
        // No prefix → unchanged.
        assert_eq!(strip_log_prefix("daemon ready"), "daemon ready");
        // Bracketed non-hex tag → leave the prefix in place so we don't
        // mangle messages that happen to start with `[Some Word]`.
        assert_eq!(
            strip_log_prefix("[Some Note] info text"),
            "[Some Note] info text"
        );
    }
}
