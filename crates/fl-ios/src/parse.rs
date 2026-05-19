//! Parsers for `xcrun devicectl` and `xcrun simctl` JSON outputs.

use fl_core::{ConnectionKind, Device, DeviceState};
use serde_json::Value;

/// Parse `xcrun devicectl list devices --json-output -` into `Device`s.
///
/// Real-world `xcrun devicectl` output often contains warning text before the JSON
/// (e.g. "Failed to load provisioning parameter list..." and a tabular preview).
/// We locate the first `{` and parse from there.
pub fn parse_devicectl_json(raw: &str) -> Vec<Device> {
    let start = match raw.find('{') {
        Some(i) => i,
        None => return Vec::new(),
    };
    let v: Value = match serde_json::from_str(&raw[start..]) {
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
    let hw = entry.get("hardwareProperties")?;
    // Prefer the ECID-based UDID (what Flutter uses) over the higher-level identifier.
    let udid = hw
        .get("udid")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| entry.get("identifier").and_then(Value::as_str).map(str::to_string))?;
    let dev_props = entry.get("deviceProperties")?;
    let name = dev_props.get("name").and_then(Value::as_str)?.to_string();

    let platform = hw
        .get("platform")
        .and_then(Value::as_str)
        .or_else(|| dev_props.get("platform").and_then(Value::as_str))
        .unwrap_or("iOS")
        .to_ascii_lowercase();
    let os_version = dev_props.get("osVersionNumber").and_then(Value::as_str).map(str::to_string);

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

    // Apple's coredevice tunnel IPv6 — this is reachable from the Mac
    // regardless of whether the cable is plugged. We use it to talk
    // directly to the device's Dart VM Service (which we've patched
    // Flutter to bind on `::0`) so hot reload survives a USB unplug.
    let tunnel_ip = conn
        .and_then(|c| c.get("tunnelIPAddress"))
        .and_then(Value::as_str)
        .map(str::to_string);

    Some(Device {
        serial: udid,
        name,
        model: hw.get("marketingName").and_then(Value::as_str).map(str::to_string),
        connection,
        state,
        ip: tunnel_ip,
        android_version: os_version,
        battery: None,
        platform: Some(platform),
    })
}

/// Look up the current coredevice tunnel IPv6 for a specific UDID by
/// shelling out to `xcrun devicectl`. Returns `None` if devicectl has no
/// tunnel for that device (e.g. it was never paired wirelessly).
pub async fn tunnel_ip_for_udid<R: fl_adb::CommandRunner>(
    xcrun: &crate::xcrun::Xcrun<R>,
    udid: &str,
) -> Option<String> {
    let devs = crate::watcher::list_apple_devices(xcrun).await;
    devs.into_iter()
        .find(|d| d.serial == udid)
        .and_then(|d| d.ip)
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
                    "identifier": "high-level-uuid-1",
                    "hardwareProperties": {
                        "udid": "00008140-001234567890",
                        "platform": "iOS",
                        "marketingName": "iPhone 15"
                    },
                    "deviceProperties": {
                        "name": "iPhone 15",
                        "osVersionNumber": "17.4.1"
                    },
                    "connectionProperties": {
                        "transportType": "wired",
                        "tunnelState": "connected"
                    }
                },
                {
                    "identifier": "high-level-uuid-2",
                    "hardwareProperties": {
                        "udid": "00008110-ABCDEF",
                        "platform": "iPadOS",
                        "marketingName": "iPad Pro"
                    },
                    "deviceProperties": {
                        "name": "iPad Pro",
                        "osVersionNumber": "17.4"
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
            "identifier":"H","hardwareProperties":{"udid":"X","platform":"iOS"},
            "deviceProperties":{"name":"iPhone"},
            "connectionProperties":{"transportType":"wired","tunnelState":"disconnected"}
        }]}}"#;
        let v = parse_devicectl_json(raw);
        assert_eq!(v[0].state, DeviceState::Offline);
    }

    #[test]
    fn parse_devicectl_json_skips_leading_warning_text() {
        let raw = "Failed to load provisioning parameter list\nName    Identifier\n---     ---\niPhone  ABC\n{\"result\":{\"devices\":[{\"identifier\":\"H\",\"hardwareProperties\":{\"udid\":\"REAL-UDID\",\"platform\":\"iOS\"},\"deviceProperties\":{\"name\":\"iPhone\"},\"connectionProperties\":{\"transportType\":\"wired\",\"tunnelState\":\"connected\"}}]}}";
        let v = parse_devicectl_json(raw);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].serial, "REAL-UDID");
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
