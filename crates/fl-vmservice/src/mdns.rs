//! mDNS browser for Dart VM Services advertised by Flutter apps.
//!
//! Flutter apps in debug/profile mode publish their VM Service via Bonjour
//! under `_dartVmService._tcp.local.` with the auth-code in TXT records.
//! This is the same mechanism Xcode and `flutter run` use for wireless
//! debugging — by listening on the LAN we can transparently fail an iOS
//! session over from USB (`usbmuxd` tunnel) to direct Wi-Fi when the
//! cable is pulled.

use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

pub const SERVICE_TYPE: &str = "_dartVmService._tcp.local.";

/// A single resolved `_dartVmService._tcp` ad on the LAN.
#[derive(Debug, Clone)]
pub struct VmServiceAd {
    pub ip: String,
    pub port: u16,
    pub auth_code: Option<String>,
    /// Fully-qualified mDNS instance name. Useful for ServiceRemoved
    /// invalidation (we don't get TXT records on removal).
    pub fullname: String,
}

impl VmServiceAd {
    /// Reconstruct the WebSocket URI Flutter would have exposed locally
    /// over USB — but pointed at the device's LAN IP instead of
    /// `127.0.0.1`. Auth code (if present) becomes the leading path
    /// segment, matching Dart VM Service's normal `/<authCode>/ws` shape.
    pub fn ws_uri(&self) -> String {
        match &self.auth_code {
            Some(code) if !code.is_empty() => {
                format!("ws://{}:{}/{}/ws", self.ip, self.port, code)
            }
            _ => format!("ws://{}:{}/ws", self.ip, self.port),
        }
    }
}

/// Shared cache populated by the background browser. Keyed by auth-code
/// so callers can look up the LAN endpoint for a specific app instance
/// without depending on hostname or instance-name conventions.
pub type AdCache = Arc<Mutex<HashMap<String, VmServiceAd>>>;

/// Snapshot the current ads so callers don't have to hold the mutex.
pub fn snapshot(cache: &AdCache) -> Vec<VmServiceAd> {
    cache
        .lock()
        .map(|c| c.values().cloned().collect())
        .unwrap_or_default()
}

/// Look up the LAN endpoint for a given auth code, if seen.
pub fn lookup(cache: &AdCache, auth_code: &str) -> Option<VmServiceAd> {
    cache.lock().ok()?.get(auth_code).cloned()
}

/// Spawn a background task that browses `_dartVmService._tcp.local.` and
/// keeps the returned [`AdCache`] up to date. Drop the JoinHandle to stop.
pub fn spawn_browser() -> anyhow::Result<(AdCache, JoinHandle<()>)> {
    let cache: AdCache = Arc::new(Mutex::new(HashMap::new()));
    let daemon = ServiceDaemon::new()?;
    let rx = daemon.browse(SERVICE_TYPE)?;
    let cache_clone = cache.clone();

    let handle = tokio::spawn(async move {
        // mdns-sd uses a sync crossbeam-style channel; poll it from a
        // tokio task with a short sleep between drains. The 250ms cadence
        // matches fl-adb's mDNS browser.
        loop {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    ServiceEvent::ServiceResolved(info) => {
                        if let Some(ad) = make_ad(&info) {
                            if let Ok(mut c) = cache_clone.lock() {
                                let key =
                                    ad.auth_code.clone().unwrap_or_else(|| ad.fullname.clone());
                                c.insert(key, ad);
                            }
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, name) => {
                        if let Ok(mut c) = cache_clone.lock() {
                            // Either keyed by authCode or fullname — drop
                            // whichever matches this instance.
                            c.retain(|_, ad| ad.fullname != name);
                        }
                    }
                    _ => {}
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    });
    Ok((cache, handle))
}

fn make_ad(info: &ResolvedService) -> Option<VmServiceAd> {
    let port = info.port;
    // Devices often advertise multiple IPv4s (real LAN + Tailscale CGNAT
    // + carrier NAT, etc.). Pick the one most likely to actually be
    // reachable end-to-end: RFC1918 private LAN > Tailscale > everything
    // else. Without this we end up trying to talk to e.g. a 100.x CGNAT
    // address when a clean 192.168.x is sitting in the same record.
    let ip = info
        .addresses
        .iter()
        .filter_map(|a| match a.to_ip_addr() {
            IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_link_local() => Some(v4),
            _ => None,
        })
        .map(|v4| (v4.to_string(), ipv4_priority(&v4)))
        .max_by_key(|(_, score)| *score)
        .map(|(s, _)| s)?;
    let auth_code = info.get_property_val_str("authCode").map(str::to_string);
    Some(VmServiceAd {
        ip,
        port,
        auth_code,
        fullname: info.fullname.clone(),
    })
}

/// Higher score = better candidate. RFC1918 private space (regular home
/// / office LAN) wins over Tailscale's 100.64.0.0/10 CGNAT range, which
/// itself beats arbitrary public addresses.
fn ipv4_priority(ip: &std::net::Ipv4Addr) -> u8 {
    let o = ip.octets();
    let is_rfc1918 =
        matches!(o, [10, ..] | [192, 168, ..]) || (o[0] == 172 && (16..=31).contains(&o[1]));
    let is_cgnat = o[0] == 100 && (64..=127).contains(&o[1]);
    if is_rfc1918 {
        100
    } else if is_cgnat {
        30
    } else {
        50
    }
}

/// Extract the auth-code path segment from a Dart VM Service WS URI.
/// `ws://127.0.0.1:63594/kvOS0zwwzO8=/ws` → `Some("kvOS0zwwzO8=")`.
/// Returns `None` if the URI has no auth code (older Flutter versions or
/// `--no-dds`).
pub fn extract_auth_code(uri: &str) -> Option<String> {
    let after_scheme = uri.split_once("://")?.1;
    let path = after_scheme.split_once('/')?.1;
    let first = path.split('/').next()?;
    if first.is_empty() || first == "ws" {
        None
    } else {
        Some(first.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_auth_code_from_standard_url() {
        assert_eq!(
            extract_auth_code("ws://127.0.0.1:63594/kvOS0zwwzO8=/ws").as_deref(),
            Some("kvOS0zwwzO8=")
        );
    }

    #[test]
    fn extract_auth_code_returns_none_when_missing() {
        assert!(extract_auth_code("ws://127.0.0.1:63594/ws").is_none());
        assert!(extract_auth_code("not-a-url").is_none());
    }

    #[test]
    fn ws_uri_uses_authcode_when_present() {
        let ad = VmServiceAd {
            ip: "192.168.1.42".into(),
            port: 5555,
            auth_code: Some("abc=".into()),
            fullname: "iPhone._dartVmService._tcp.local.".into(),
        };
        assert_eq!(ad.ws_uri(), "ws://192.168.1.42:5555/abc=/ws");
    }

    #[test]
    fn ws_uri_falls_back_to_bare_ws_path() {
        let ad = VmServiceAd {
            ip: "192.168.1.42".into(),
            port: 5555,
            auth_code: None,
            fullname: "iPhone._dartVmService._tcp.local.".into(),
        };
        assert_eq!(ad.ws_uri(), "ws://192.168.1.42:5555/ws");
    }

    #[test]
    fn snapshot_and_lookup_round_trip() {
        let cache: AdCache = Arc::new(Mutex::new(HashMap::new()));
        let ad = VmServiceAd {
            ip: "10.0.0.5".into(),
            port: 60000,
            auth_code: Some("auth".into()),
            fullname: "x._dartVmService._tcp.local.".into(),
        };
        cache.lock().unwrap().insert("auth".to_string(), ad.clone());
        let snap = snapshot(&cache);
        assert_eq!(snap.len(), 1);
        let found = lookup(&cache, "auth").unwrap();
        assert_eq!(found.ip, "10.0.0.5");
        assert!(lookup(&cache, "missing").is_none());
    }
}
