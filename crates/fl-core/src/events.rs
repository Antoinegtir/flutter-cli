//! Events flowing through the application's central mpsc channel.

use serde::{Deserialize, Serialize};

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
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
