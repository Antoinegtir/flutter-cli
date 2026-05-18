//! Parsers for `adb devices -l` and `adb shell ip -f inet addr show wlan0`.

use fl_core::{ConnectionKind, Device, DeviceState};

/// Parse the output of `adb devices -l`.
/// Lines like:
///   `ABC123  device usb:1-2 product:foo model:Pixel_8 device:husky transport_id:1`
///   `192.168.1.42:5555  device product:foo model:Pixel_8 device:husky transport_id:2`
pub fn parse_devices_l(stdout: &str) -> Vec<Device> {
    let mut out = Vec::new();
    for raw_line in stdout.lines().skip(1) {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let serial = match fields.next() {
            Some(s) => s.to_string(),
            None => continue,
        };
        let state = match fields.next() {
            Some("device") => DeviceState::Online,
            Some("offline") => DeviceState::Offline,
            Some("unauthorized") => DeviceState::Unauthorized,
            Some("connecting") => DeviceState::Connecting,
            _ => continue,
        };

        let mut model: Option<String> = None;
        for kv in fields {
            if let Some(rest) = kv.strip_prefix("model:") {
                model = Some(rest.replace('_', " "));
            }
        }

        let (connection, ip) = if serial.contains(':') && serial.contains('.') {
            let ip = serial.split(':').next().map(str::to_string);
            (ConnectionKind::Wifi, ip)
        } else {
            (ConnectionKind::Usb, None)
        };

        out.push(Device {
            name: model.clone().unwrap_or_else(|| serial.clone()),
            serial,
            model,
            connection,
            state,
            ip,
            android_version: None,
            battery: None,
            platform: Some("android".into()),
        });
    }
    out
}

/// Parse `ip -f inet addr show wlan0` and return the first non-loopback IPv4.
/// Sample line:
///   `    inet 192.168.1.42/24 brd 192.168.1.255 scope global wlan0`
pub fn parse_wlan_ip(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("inet ") {
            let candidate = rest.split('/').next()?.trim();
            if !candidate.starts_with("127.") {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICES_BOTH: &str = "List of devices attached\n\
        ABC123              device usb:1-2 product:husky model:Pixel_8 device:husky transport_id:1\n\
        192.168.1.42:5555   device product:husky model:Pixel_8 device:husky transport_id:2\n";

    #[test]
    fn parses_usb_and_wifi_devices() {
        let v = parse_devices_l(DEVICES_BOTH);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].serial, "ABC123");
        assert_eq!(v[0].connection, ConnectionKind::Usb);
        assert_eq!(v[0].model.as_deref(), Some("Pixel 8"));
        assert_eq!(v[1].serial, "192.168.1.42:5555");
        assert_eq!(v[1].connection, ConnectionKind::Wifi);
        assert_eq!(v[1].ip.as_deref(), Some("192.168.1.42"));
    }

    #[test]
    fn parses_offline_device() {
        let s = "List of devices attached\nXYZ offline transport_id:5\n";
        let v = parse_devices_l(s);
        assert_eq!(v[0].state, DeviceState::Offline);
    }

    #[test]
    fn empty_list_returns_empty_vec() {
        assert!(parse_devices_l("List of devices attached\n").is_empty());
    }

    #[test]
    fn finds_wlan_ip() {
        let s = "3: wlan0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 ...\n\
                 inet 192.168.1.42/24 brd 192.168.1.255 scope global wlan0\n";
        assert_eq!(parse_wlan_ip(s).as_deref(), Some("192.168.1.42"));
    }

    #[test]
    fn ignores_loopback() {
        let s = "    inet 127.0.0.1/8 scope host lo\n";
        assert!(parse_wlan_ip(s).is_none());
    }
}
