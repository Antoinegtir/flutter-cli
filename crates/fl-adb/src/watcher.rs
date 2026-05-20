//! Track device add/remove via the ADB server protocol on 127.0.0.1:5037.

use anyhow::{anyhow, Context};
use fl_core::{ConnectionKind, Device, DeviceEvent, DeviceState};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::Sender;

const ADB_HOST: &str = "127.0.0.1:5037";

pub fn parse_track_payload(payload: &str) -> Vec<Device> {
    payload
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let serial = parts.next()?.trim();
            if serial.is_empty() {
                return None;
            }
            let state = match parts.next().unwrap_or("").trim() {
                "device" => DeviceState::Online,
                "offline" => DeviceState::Offline,
                "unauthorized" => DeviceState::Unauthorized,
                "connecting" => DeviceState::Connecting,
                _ => return None,
            };
            let (connection, ip) = if serial.contains(':') && serial.contains('.') {
                (
                    ConnectionKind::Wifi,
                    serial.split(':').next().map(str::to_string),
                )
            } else {
                (ConnectionKind::Usb, None)
            };
            Some(Device {
                serial: serial.to_string(),
                name: serial.to_string(),
                model: None,
                connection,
                state,
                ip,
                android_version: None,
                battery: None,
                platform: Some("android".into()),
            })
        })
        .collect()
}

/// Compute the diff between previous and current device sets, emitting events.
pub fn diff_devices(prev: &HashMap<String, Device>, cur: &[Device]) -> Vec<DeviceEvent> {
    let cur_map: HashMap<&str, &Device> = cur.iter().map(|d| (d.serial.as_str(), d)).collect();
    let mut events = Vec::new();
    for new in cur {
        if !prev.contains_key(&new.serial) {
            events.push(DeviceEvent::Discovered(new.clone()));
        }
    }
    for old_serial in prev.keys() {
        if !cur_map.contains_key(old_serial.as_str()) {
            let old = &prev[old_serial];
            if old.connection == ConnectionKind::Usb {
                events.push(DeviceEvent::UsbDisconnected {
                    serial: old_serial.clone(),
                });
            } else {
                events.push(DeviceEvent::Lost {
                    serial: old_serial.clone(),
                });
            }
        }
    }
    events
}

async fn read_hex_len(stream: &mut TcpStream) -> anyhow::Result<usize> {
    let mut buf = [0u8; 4];
    stream
        .read_exact(&mut buf)
        .await
        .context("reading hex length")?;
    let s = std::str::from_utf8(&buf).context("hex length is not utf8")?;
    usize::from_str_radix(s, 16).map_err(|e| anyhow!("bad hex length `{s}`: {e}"))
}

async fn read_payload(stream: &mut TcpStream, len: usize) -> anyhow::Result<String> {
    let mut buf = vec![0u8; len];
    stream
        .read_exact(&mut buf)
        .await
        .context("reading payload")?;
    String::from_utf8(buf).context("payload not utf8")
}

/// Connect to the adb server, send `host:track-devices`, and forward diffs.
pub async fn track_devices(tx: Sender<DeviceEvent>) -> anyhow::Result<()> {
    let mut stream = TcpStream::connect(ADB_HOST)
        .await
        .context("connecting to adb server")?;
    let cmd = "host:track-devices";
    let header = format!("{:04x}", cmd.len());
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(cmd.as_bytes()).await?;

    let mut status = [0u8; 4];
    stream.read_exact(&mut status).await?;
    if &status != b"OKAY" {
        return Err(anyhow!("adb server refused track-devices: {:?}", status));
    }

    let mut prev: HashMap<String, Device> = HashMap::new();
    loop {
        let len = read_hex_len(&mut stream).await?;
        let payload = read_payload(&mut stream, len).await?;
        let cur = parse_track_payload(&payload);
        let cur_map: HashMap<String, Device> =
            cur.iter().cloned().map(|d| (d.serial.clone(), d)).collect();
        for ev in diff_devices(&prev, &cur) {
            tx.send(ev).await.ok();
        }
        prev = cur_map;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_track_payload() {
        let p = "ABC123\tdevice\n192.168.1.42:5555\tdevice\n";
        let v = parse_track_payload(p);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].connection, ConnectionKind::Usb);
        assert_eq!(v[1].connection, ConnectionKind::Wifi);
    }

    #[test]
    fn diff_emits_discovered_for_new_serial() {
        let prev = HashMap::new();
        let cur = parse_track_payload("ABC\tdevice\n");
        let evs = diff_devices(&prev, &cur);
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], DeviceEvent::Discovered(_)));
    }

    #[test]
    fn diff_emits_usb_disconnected_for_lost_usb_serial() {
        let mut prev = HashMap::new();
        prev.insert(
            "ABC".into(),
            Device {
                serial: "ABC".into(),
                name: "ABC".into(),
                model: None,
                connection: ConnectionKind::Usb,
                state: DeviceState::Online,
                ip: None,
                android_version: None,
                battery: None,
                platform: None,
            },
        );
        let evs = diff_devices(&prev, &[]);
        assert_eq!(evs.len(), 1);
        assert!(matches!(&evs[0], DeviceEvent::UsbDisconnected { serial } if serial == "ABC"));
    }

    #[test]
    fn diff_emits_lost_for_lost_wifi_serial() {
        let mut prev = HashMap::new();
        prev.insert(
            "1.2.3.4:5555".into(),
            Device {
                serial: "1.2.3.4:5555".into(),
                name: "1.2.3.4:5555".into(),
                model: None,
                connection: ConnectionKind::Wifi,
                state: DeviceState::Online,
                ip: Some("1.2.3.4".into()),
                android_version: None,
                battery: None,
                platform: None,
            },
        );
        let evs = diff_devices(&prev, &[]);
        assert!(matches!(&evs[0], DeviceEvent::Lost { serial } if serial == "1.2.3.4:5555"));
    }
}
