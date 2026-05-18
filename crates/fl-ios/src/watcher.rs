//! Polling watcher for Apple devices via `xcrun`.

use crate::parse::{parse_devicectl_json, parse_simctl_json};
use crate::xcrun::Xcrun;
use fl_adb::CommandRunner;
use fl_core::{Device, DeviceEvent};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

/// Single-shot snapshot of Apple devices (devicectl + simctl combined).
pub async fn list_apple_devices<R: CommandRunner>(xcrun: &Xcrun<R>) -> Vec<Device> {
    let mut devs = Vec::new();
    if let Ok(j) = xcrun.devicectl_list().await {
        devs.extend(parse_devicectl_json(&j));
    }
    if let Ok(j) = xcrun.simctl_list().await {
        devs.extend(parse_simctl_json(&j));
    }
    devs
}

/// Compute the diff between previous and current Apple device sets.
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
            events.push(DeviceEvent::Lost { serial: old_serial.clone() });
        }
    }
    events
}

/// Long-running polling loop. Polls every 3 seconds and emits Discovered/Lost diffs.
pub async fn watch_apple_devices<R>(xcrun: Xcrun<R>, tx: Sender<DeviceEvent>)
where
    R: CommandRunner + Send + Sync + 'static,
{
    let mut prev: HashMap<String, Device> = HashMap::new();
    loop {
        let cur = list_apple_devices(&xcrun).await;
        let cur_map: HashMap<String, Device> = cur.iter().cloned().map(|d| (d.serial.clone(), d)).collect();
        for ev in diff_devices(&prev, &cur) {
            tx.send(ev).await.ok();
        }
        prev = cur_map;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fl_adb::{CommandOutput, MockRunner};
    use fl_core::{ConnectionKind, DeviceState};

    fn dev(serial: &str) -> Device {
        Device {
            serial: serial.into(),
            name: serial.into(),
            model: None,
            connection: ConnectionKind::Usb,
            state: DeviceState::Online,
            ip: None,
            android_version: None,
            battery: None,
            platform: Some("ios".into()),
        }
    }

    #[test]
    fn diff_emits_discovered_for_new_serial() {
        let prev = HashMap::new();
        let cur = vec![dev("A")];
        let evs = diff_devices(&prev, &cur);
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], DeviceEvent::Discovered(_)));
    }

    #[test]
    fn diff_emits_lost_for_dropped_serial() {
        let mut prev = HashMap::new();
        prev.insert("A".into(), dev("A"));
        let cur: Vec<Device> = Vec::new();
        let evs = diff_devices(&prev, &cur);
        assert_eq!(evs.len(), 1);
        assert!(matches!(&evs[0], DeviceEvent::Lost { serial } if serial == "A"));
    }

    #[tokio::test]
    async fn list_apple_devices_combines_devicectl_and_simctl() {
        let m = MockRunner::new();
        m.expect(
            "xcrun devicectl list devices --json-output -",
            CommandOutput::ok(r#"{"result":{"devices":[{
                "identifier":"P","deviceProperties":{"name":"iPhone","platform":"iOS"},
                "connectionProperties":{"transportType":"wired","tunnelState":"connected"}
            }]}}"#),
        );
        m.expect(
            "xcrun simctl list devices --json",
            CommandOutput::ok(r#"{"devices":{"r":[
                {"udid":"S","name":"Sim","state":"Booted","isAvailable":true}
            ]}}"#),
        );
        let x = Xcrun::new(m);
        let all = list_apple_devices(&x).await;
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|d| d.serial == "P"));
        assert!(all.iter().any(|d| d.serial == "S"));
    }
}
