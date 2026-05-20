//! mDNS browser for adb wireless debugging services.
//!
//! Watches `_adb-tls-connect._tcp.local.` (Android 11+) and `_adb._tcp.local.`,
//! filters by device name, and forwards new IPv4 addresses as Reconnect inputs.

use crate::reconnect::Input as ReconnectInput;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::net::IpAddr;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;

pub const SERVICE_TYPES: &[&str] = &["_adb-tls-connect._tcp.local.", "_adb._tcp.local."];

/// Extracts the first non-loopback IPv4 from a resolved service.
/// Returns `None` if no suitable address is present.
pub fn pick_ipv4(info: &ServiceInfo) -> Option<String> {
    info.get_addresses().iter().find_map(|a| match a {
        IpAddr::V4(v4) if !v4.is_loopback() => Some(v4.to_string()),
        _ => None,
    })
}

/// Returns true if the service announcement is for the target device.
/// Matches when:
///  - the `name` TXT property equals `device_name` (case-insensitive), OR
///  - the service `fullname` contains the device name slug.
pub fn matches_device(info: &ServiceInfo, device_name: &str) -> bool {
    let target = device_name.trim().to_ascii_lowercase().replace(' ', "_");
    if let Some(name) = info.get_property_val_str("name") {
        if name.trim().eq_ignore_ascii_case(device_name) {
            return true;
        }
    }
    info.get_fullname().to_ascii_lowercase().contains(&target)
}

/// Start the mDNS browser; forward `IpDiscovered` to `reconnect_tx`.
/// Returns the spawned task; dropping it stops the browser.
pub fn spawn(
    device_name: String,
    reconnect_tx: Sender<ReconnectInput>,
) -> anyhow::Result<JoinHandle<()>> {
    let daemon = ServiceDaemon::new()?;
    let mut receivers = Vec::with_capacity(SERVICE_TYPES.len());
    for svc in SERVICE_TYPES {
        receivers.push(daemon.browse(svc)?);
    }

    let handle = tokio::spawn(async move {
        loop {
            for rx in &receivers {
                while let Ok(ev) = rx.try_recv() {
                    if let ServiceEvent::ServiceResolved(info) = ev {
                        if !matches_device(&info, &device_name) {
                            continue;
                        }
                        if let Some(ip) = pick_ipv4(&info) {
                            reconnect_tx
                                .send(ReconnectInput::IpDiscovered { new_ip: ip })
                                .await
                                .ok();
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    });
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Test helper: build a `ServiceInfo` using the 0.11.x constructor.
    /// `addrs` is a comma-joined string (e.g. "127.0.0.1,192.168.1.42").
    /// `name_prop` is optional; when `Some`, it is inserted into the TXT properties as "name".
    fn info(my_name: &str, name_prop: Option<&str>, addrs: &[&str]) -> ServiceInfo {
        let addr_str = addrs.join(",");
        let props: Option<HashMap<String, String>> = name_prop.map(|n| {
            let mut m = HashMap::new();
            m.insert("name".to_string(), n.to_string());
            m
        });
        ServiceInfo::new(
            "_adb-tls-connect._tcp.local.",
            my_name,
            "host.local.",
            addr_str.as_str(),
            5555,
            props,
        )
        .unwrap()
    }

    #[test]
    fn pick_ipv4_picks_first_non_loopback() {
        let i = info("adb-xyz", None, &["127.0.0.1", "192.168.1.42"]);
        assert_eq!(pick_ipv4(&i).as_deref(), Some("192.168.1.42"));
    }

    #[test]
    fn pick_ipv4_returns_none_when_only_loopback() {
        let i = info("adb-xyz", None, &["127.0.0.1"]);
        assert!(pick_ipv4(&i).is_none());
    }

    #[test]
    fn matches_device_via_property() {
        let i = info("adb-xyz", Some("Pixel 8"), &["192.168.1.42"]);
        assert!(matches_device(&i, "Pixel 8"));
        assert!(matches_device(&i, "pixel 8"));
        assert!(!matches_device(&i, "Galaxy S24"));
    }

    #[test]
    fn matches_device_via_fullname_slug() {
        let i = info("adb-Pixel_8-deadbeef", None, &["192.168.1.42"]);
        assert!(matches_device(&i, "Pixel 8"));
    }
}
