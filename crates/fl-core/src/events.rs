//! Events flowing through the application's central mpsc channel.

use serde::{Deserialize, Serialize};
use clap::ValueEnum;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AppEvent {
    Device(DeviceEvent),
    Flutter(FlutterEvent),
    Vm(VmEvent),
    Key(KeyEvent),
    Tick,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeviceEvent {
    Discovered(Device),
    Lost { serial: String },
    UsbDisconnected { serial: String },
    WifiPaired { serial: String, ip: String, port: u16 },
    WifiReconnecting { attempt: u32 },
    WifiReconnected,
    IpChanged { serial: String, old_ip: String, new_ip: String },
    SessionState { serial: String, state: DeviceSessionState },
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub serial: String,
    pub name: String,
    pub model: Option<String>,
    pub connection: ConnectionKind,
    pub state: DeviceState,
    pub ip: Option<String>,
    pub android_version: Option<String>,
    pub battery: Option<u8>,
    pub platform: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConnectionKind {
    Usb,
    Wifi,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeviceState {
    Online,
    Offline,
    Unauthorized,
    Connecting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FlutterEvent {
    DaemonReady,
    AppStarted { app_id: String, vm_service_uri: String },
    Log { level: LogLevel, message: String },
    Progress { id: String, message: String, finished: bool },
    Stopped { exit_code: Option<i32> },
    Error(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VmEvent {
    Connected,
    Disconnected,
    Stdout(String),
    Stderr(String),
    IsolateEvent(String),
    FrameTiming { ui_micros: u64, raster_micros: u64 },
    GcStats { used_mb: f64, total_mb: f64 },
    ExtensionResult { id: u64, ok: bool, error: Option<String> },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum KeyEvent {
    Char(char),
    Enter,
    Esc,
    Tab,
    Ctrl(char),
    Up,
    Down,
    PageUp,
    PageDown,
    Backspace,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum BuildMode {
    Debug,
    Profile,
    Release,
}

impl BuildMode {
    pub fn flutter_flag(self) -> &'static str {
        match self {
            BuildMode::Debug => "--debug",
            BuildMode::Profile => "--profile",
            BuildMode::Release => "--release",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum BuildTarget {
    Apk,
    Aab,
    Ios,
    Web,
}

impl BuildTarget {
    pub fn flutter_arg(self) -> &'static str {
        match self {
            BuildTarget::Apk => "apk",
            BuildTarget::Aab => "appbundle",
            BuildTarget::Ios => "ios",
            BuildTarget::Web => "web",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TestResult {
    Success,
    Failure,
    Error,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TestEvent {
    SuiteStart { path: String },
    TestStarted { id: u64, name: String },
    TestDone { id: u64, name: String, result: TestResult, duration_ms: u64 },
    Error { id: Option<u64>, message: String, stack: Option<String> },
    AllDone { success: bool, passed: u32, failed: u32, skipped: u32 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DoctorStatus {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DoctorEvent {
    Section { status: DoctorStatus, title: String, details: Vec<String> },
    Done,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PubDepKind {
    Direct,
    Dev,
    Transitive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutdatedRow {
    pub package: String,
    pub current: String,
    pub upgradable: String,
    pub resolvable: String,
    pub latest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PubTreeNode {
    pub name: String,
    pub version: String,
    pub kind: PubDepKind,
    pub children: Vec<PubTreeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PubEvent {
    Resolving,
    Got {
        added: Vec<String>,
        removed: Vec<String>,
        modified: Vec<(String, String, String)>,
    },
    Outdated { rows: Vec<OutdatedRow> },
    Deps { tree: PubTreeNode },
    Log { level: LogLevel, message: String },
    Done { success: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CleanEvent {
    Probing,
    Cleaning { path: String },
    Done { freed_bytes: u64, paths: Vec<String> },
    Error(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceSessionState {
    Connecting,
    Ready,
    Reloading,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceSessionSummary {
    pub serial: String,
    pub short_name: String,
    pub display_name: String,
    pub connection: ConnectionKind,
    pub ip: Option<String>,
    pub state: DeviceSessionState,
    pub platform: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appevent_roundtrips_through_json() {
        let original = AppEvent::Device(DeviceEvent::WifiPaired {
            serial: "ABC123".into(),
            ip: "192.168.1.42".into(),
            port: 5555,
        });
        let json = serde_json::to_string(&original).unwrap();
        let back: AppEvent = serde_json::from_str(&json).unwrap();
        match back {
            AppEvent::Device(DeviceEvent::WifiPaired { serial, ip, port }) => {
                assert_eq!(serial, "ABC123");
                assert_eq!(ip, "192.168.1.42");
                assert_eq!(port, 5555);
            }
            _ => panic!("variant mismatch"),
        }
    }

    #[test]
    fn device_equality_is_value_based() {
        let a = Device {
            serial: "S1".into(),
            name: "Pixel 8".into(),
            model: Some("Pixel 8".into()),
            connection: ConnectionKind::Usb,
            state: DeviceState::Online,
            ip: None,
            android_version: Some("14".into()),
            battery: Some(90),
            platform: None,
        };
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn ipchanged_roundtrips_through_json() {
        let original = AppEvent::Device(DeviceEvent::IpChanged {
            serial: "1.2.3.4:5555".into(),
            old_ip: "1.2.3.4".into(),
            new_ip: "10.0.0.5".into(),
        });
        let json = serde_json::to_string(&original).unwrap();
        let back: AppEvent = serde_json::from_str(&json).unwrap();
        match back {
            AppEvent::Device(DeviceEvent::IpChanged { serial, old_ip, new_ip }) => {
                assert_eq!(serial, "1.2.3.4:5555");
                assert_eq!(old_ip, "1.2.3.4");
                assert_eq!(new_ip, "10.0.0.5");
            }
            _ => panic!("variant mismatch"),
        }
    }

    #[test]
    fn build_mode_flag_mapping() {
        assert_eq!(BuildMode::Debug.flutter_flag(), "--debug");
        assert_eq!(BuildMode::Profile.flutter_flag(), "--profile");
        assert_eq!(BuildMode::Release.flutter_flag(), "--release");
    }

    #[test]
    fn build_target_arg_mapping() {
        assert_eq!(BuildTarget::Apk.flutter_arg(), "apk");
        assert_eq!(BuildTarget::Aab.flutter_arg(), "appbundle");
        assert_eq!(BuildTarget::Ios.flutter_arg(), "ios");
        assert_eq!(BuildTarget::Web.flutter_arg(), "web");
    }

    #[test]
    fn cleanevent_done_roundtrips() {
        let original = CleanEvent::Done {
            freed_bytes: 12345,
            paths: vec!["build/".into(), ".dart_tool/".into()],
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: CleanEvent = serde_json::from_str(&json).unwrap();
        match back {
            CleanEvent::Done { freed_bytes, paths } => {
                assert_eq!(freed_bytes, 12345);
                assert_eq!(paths.len(), 2);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn session_state_roundtrips_through_json() {
        let ev = AppEvent::Device(DeviceEvent::SessionState {
            serial: "ABC".into(),
            state: DeviceSessionState::Ready,
        });
        let json = serde_json::to_string(&ev).unwrap();
        let back: AppEvent = serde_json::from_str(&json).unwrap();
        match back {
            AppEvent::Device(DeviceEvent::SessionState { serial, state }) => {
                assert_eq!(serial, "ABC");
                assert_eq!(state, DeviceSessionState::Ready);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn device_platform_roundtrips_through_json() {
        let d = Device {
            serial: "X".into(),
            name: "x".into(),
            model: None,
            connection: ConnectionKind::Wifi,
            state: DeviceState::Online,
            ip: None,
            android_version: None,
            battery: None,
            platform: Some("ios".into()),
        };
        let j = serde_json::to_string(&d).unwrap();
        let back: Device = serde_json::from_str(&j).unwrap();
        assert_eq!(back.platform.as_deref(), Some("ios"));
    }

    #[test]
    fn device_session_summary_equality() {
        let s = DeviceSessionSummary {
            serial: "S".into(),
            short_name: "short".into(),
            display_name: "Pixel 8".into(),
            connection: ConnectionKind::Wifi,
            ip: Some("1.2.3.4".into()),
            state: DeviceSessionState::Connecting,
            platform: None,
        };
        let t = s.clone();
        assert_eq!(s, t);
    }
}
