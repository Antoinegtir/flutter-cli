//! Pre-pair a USB device for WiFi adb so the session can survive unplugging.

use crate::parse::parse_wlan_ip;
use crate::runner::CommandRunner;
use anyhow::{anyhow, Context};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiTarget {
    pub ip: String,
    pub port: u16,
}

impl WifiTarget {
    pub fn serial(&self) -> String {
        format!("{}:{}", self.ip, self.port)
    }
}

/// Given a USB-attached device serial, enable tcpip mode, fetch its WiFi IP,
/// and `adb connect` to it. Returns the WiFi target on success.
pub async fn pre_pair_wifi<R: CommandRunner + ?Sized>(
    runner: &R,
    usb_serial: &str,
    port: u16,
) -> anyhow::Result<WifiTarget> {
    let port_s = port.to_string();
    let tcpip = runner
        .run("adb", &["-s", usb_serial, "tcpip", &port_s])
        .await
        .context("adb tcpip failed to spawn")?;
    if tcpip.status != 0 {
        return Err(anyhow!("adb tcpip exited {}: {}", tcpip.status, tcpip.stderr.trim()));
    }

    let ip_out = runner
        .run("adb", &["-s", usb_serial, "shell", "ip", "-f", "inet", "addr", "show", "wlan0"])
        .await
        .context("adb shell ip addr failed to spawn")?;
    let ip = parse_wlan_ip(&ip_out.stdout).ok_or_else(|| anyhow!("no wlan0 IPv4 found"))?;

    let target = format!("{ip}:{port}");
    let connect = runner
        .run("adb", &["connect", &target])
        .await
        .context("adb connect failed to spawn")?;
    if connect.status != 0 || connect.stdout.contains("failed to connect") {
        return Err(anyhow!("adb connect to {target} failed: {}{}", connect.stdout.trim(), connect.stderr.trim()));
    }

    Ok(WifiTarget { ip, port })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{CommandOutput, MockRunner};

    fn happy_runner() -> MockRunner {
        let m = MockRunner::new();
        m.expect("adb -s ABC123 tcpip 5555", CommandOutput::ok("restarting in TCP mode port: 5555\n"));
        m.expect(
            "adb -s ABC123 shell ip -f inet addr show wlan0",
            CommandOutput::ok("    inet 192.168.1.42/24 brd 192.168.1.255 scope global wlan0\n"),
        );
        m.expect("adb connect 192.168.1.42:5555", CommandOutput::ok("connected to 192.168.1.42:5555\n"));
        m
    }

    #[tokio::test]
    async fn pairs_successfully_on_happy_path() {
        let r = happy_runner();
        let t = pre_pair_wifi(&r, "ABC123", 5555).await.unwrap();
        assert_eq!(t.ip, "192.168.1.42");
        assert_eq!(t.port, 5555);
        assert_eq!(t.serial(), "192.168.1.42:5555");
        assert_eq!(r.calls().len(), 3);
    }

    #[tokio::test]
    async fn fails_when_no_ip_returned() {
        let r = MockRunner::new();
        r.expect("adb -s ABC123 tcpip 5555", CommandOutput::ok(""));
        r.expect("adb -s ABC123 shell ip -f inet addr show wlan0", CommandOutput::ok(""));
        let err = pre_pair_wifi(&r, "ABC123", 5555).await.unwrap_err();
        assert!(err.to_string().contains("no wlan0 IPv4"));
    }

    #[tokio::test]
    async fn fails_when_connect_says_failed() {
        let r = MockRunner::new();
        r.expect("adb -s ABC123 tcpip 5555", CommandOutput::ok(""));
        r.expect(
            "adb -s ABC123 shell ip -f inet addr show wlan0",
            CommandOutput::ok("inet 10.0.0.5/24 scope global wlan0\n"),
        );
        r.expect(
            "adb connect 10.0.0.5:5555",
            CommandOutput {
                stdout: "failed to connect to 10.0.0.5:5555\n".into(),
                stderr: String::new(),
                status: 0,
            },
        );
        let err = pre_pair_wifi(&r, "ABC123", 5555).await.unwrap_err();
        assert!(err.to_string().contains("failed"));
    }
}
