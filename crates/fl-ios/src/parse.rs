//! Parsers for `xcrun devicectl` and `xcrun simctl` JSON outputs.

use fl_core::{ConnectionKind, Device, DeviceState};
use serde_json::Value;

/// Parse `xcrun devicectl list devices --json-output -` into `Device`s.
pub fn parse_devicectl_json(raw: &str) -> Vec<Device> {
    let v: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let devices = match v.get("result").and_then(|r| r.get("devices")).and_then(Value::as_array) {
        Some(d) => d,
        None => return Vec::new(),
    };
    devices.iter().filter_map(parse_devicectl_entry).collect()
}

fn parse_devicectl_entry(entry: &Value) -> Option<Device> {
    let identifier = entry.get("identifier").and_then(Value::as_str)?.to_string();
    let props = entry.get("deviceProperties")?;
    let name = props.get("name").and_then(Value::as_str)?.to_string();
    let platform_raw = props.get("platform").and_then(Value::as_str).unwrap_or("iOS").to_string();
    let platform = platform_raw.to_ascii_lowercase();
    let os_version = props.get("osVersionNumber").and_then(Value::as_str).map(str::to_string);

    let conn = entry.get("connectionProperties");
    let connection = match conn.and_then(|c| c.get("transportType")).and_then(Value::as_str) {
        Some("wired") => ConnectionKind::Usb,
        _ => ConnectionKind::Wifi,
    };
    let tunnel_connected = conn
        .and_then(|c| c.get("tunnelState"))
        .and_then(Value::as_str)
        .map(|s| s == "connected")
        .unwrap_or(true);
    let state = if tunnel_connected { DeviceState::Online } else { DeviceState::Offline };

    Some(Device {
        serial: identifier.clone(),
        name,
        model: None,
        connection,
        state,
        ip: None,
        android_version: os_version,
        battery: None,
        platform: Some(platform),
    })
}

/// Parse `xcrun simctl list devices --json` into `Device`s.
/// Filters to `state == "Booted" && isAvailable`.
pub fn parse_simctl_json(raw: &str) -> Vec<Device> {
    let v: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let runtimes = match v.get("devices").and_then(Value::as_object) {
        Some(o) => o,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for (_runtime, list) in runtimes {
        let Some(arr) = list.as_array() else { continue };
        for entry in arr {
            let state = entry.get("state").and_then(Value::as_str).unwrap_or("");
            let available = entry.get("isAvailable").and_then(Value::as_bool).unwrap_or(false);
            if state != "Booted" || !available {
                continue;
            }
            let Some(udid) = entry.get("udid").and_then(Value::as_str) else { continue };
            let name = entry.get("name").and_then(Value::as_str).unwrap_or(udid).to_string();
            out.push(Device {
                serial: udid.to_string(),
                name,
                model: None,
                connection: ConnectionKind::Usb,
                state: DeviceState::Online,
                ip: None,
                android_version: None,
                battery: None,
                platform: Some("ios-simulator".into()),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_DEVICES: &str = r#"{
        "result": {
            "devices": [
                {
                    "identifier": "00008140-001234567890",
                    "deviceProperties": {
                        "name": "iPhone 15",
                        "osVersionNumber": "17.4.1",
                        "platform": "iOS"
                    },
                    "connectionProperties": {
                        "transportType": "wired",
                        "tunnelState": "connected"
                    }
                },
                {
                    "identifier": "00008110-ABCDEF",
                    "deviceProperties": {
                        "name": "iPad Pro",
                        "osVersionNumber": "17.4",
                        "platform": "iPadOS"
                    },
                    "connectionProperties": {
                        "transportType": "wireless",
                        "tunnelState": "connected"
                    }
                }
            ]
        }
    }"#;

    #[test]
    fn parse_devicectl_json_two_devices() {
        let v = parse_devicectl_json(TWO_DEVICES);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].serial, "00008140-001234567890");
        assert_eq!(v[0].name, "iPhone 15");
        assert_eq!(v[0].connection, ConnectionKind::Usb);
        assert_eq!(v[0].platform.as_deref(), Some("ios"));
        assert_eq!(v[0].android_version.as_deref(), Some("17.4.1"));
        assert_eq!(v[1].connection, ConnectionKind::Wifi);
        assert_eq!(v[1].platform.as_deref(), Some("ipados"));
    }

    #[test]
    fn parse_devicectl_json_developer_mode_disabled_marks_offline() {
        let raw = r#"{"result":{"devices":[{
            "identifier":"X","deviceProperties":{"name":"iPhone","platform":"iOS"},
            "connectionProperties":{"transportType":"wired","tunnelState":"disconnected"}
        }]}}"#;
        let v = parse_devicectl_json(raw);
        assert_eq!(v[0].state, DeviceState::Offline);
    }

    #[test]
    fn parse_devicectl_json_malformed_returns_empty() {
        assert!(parse_devicectl_json("").is_empty());
        assert!(parse_devicectl_json("not json").is_empty());
        assert!(parse_devicectl_json(r#"{"unrelated": true}"#).is_empty());
    }

    const SIMCTL_TWO: &str = r#"{
        "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-17-4": [
                {
                    "udid": "BOOTED-1111",
                    "name": "iPhone 15 Pro",
                    "state": "Booted",
                    "isAvailable": true
                },
                {
                    "udid": "SHUTDOWN-2222",
                    "name": "iPhone 14",
                    "state": "Shutdown",
                    "isAvailable": true
                }
            ]
        }
    }"#;

    #[test]
    fn parse_simctl_json_filters_shutdown() {
        let v = parse_simctl_json(SIMCTL_TWO);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].serial, "BOOTED-1111");
        assert_eq!(v[0].name, "iPhone 15 Pro");
        assert_eq!(v[0].platform.as_deref(), Some("ios-simulator"));
    }

    #[test]
    fn parse_simctl_json_unavailable_is_filtered() {
        let raw = r#"{"devices":{"r":[{"udid":"x","name":"x","state":"Booted","isAvailable":false}]}}"#;
        assert!(parse_simctl_json(raw).is_empty());
    }

    #[test]
    fn parse_simctl_json_malformed_returns_empty() {
        assert!(parse_simctl_json("").is_empty());
        assert!(parse_simctl_json("nope").is_empty());
    }
}
