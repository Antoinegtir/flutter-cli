//! `fl devices` — list attached devices using fl-adb.

use anyhow::Context;
use fl_adb::{parse_devices_l, CommandRunner, TokioRunner};
use fl_core::{ConnectionKind, Device, DeviceState};

pub async fn run() -> anyhow::Result<()> {
    let runner = TokioRunner;
    let out = runner.run("adb", &["devices", "-l"]).await.context("running `adb devices -l`")?;
    if out.status != 0 {
        anyhow::bail!("adb exited with status {}: {}", out.status, out.stderr.trim());
    }
    let mut devices = parse_devices_l(&out.stdout);
    enrich(&runner, &mut devices).await;
    print_table(&devices);
    Ok(())
}

async fn enrich<R: CommandRunner + ?Sized>(runner: &R, devices: &mut [Device]) {
    for d in devices.iter_mut() {
        if let Ok(o) = runner.run("adb", &["-s", &d.serial, "shell", "getprop", "ro.build.version.release"]).await {
            let v = o.stdout.trim();
            if !v.is_empty() { d.android_version = Some(v.into()); }
        }
        if let Ok(o) = runner.run("adb", &["-s", &d.serial, "shell", "dumpsys", "battery"]).await {
            for line in o.stdout.lines() {
                if let Some(rest) = line.trim().strip_prefix("level:") {
                    if let Ok(n) = rest.trim().parse() {
                        d.battery = Some(n);
                        break;
                    }
                }
            }
        }
    }
}

fn print_table(devices: &[Device]) {
    if devices.is_empty() {
        println!("(no devices)");
        return;
    }
    println!("{:<24} {:<22} {:<7} {:<16} {:<8} {:<8}", "NAME", "SERIAL", "CONN", "IP", "ANDROID", "BAT");
    for d in devices {
        let conn = match d.connection {
            ConnectionKind::Usb => "USB",
            ConnectionKind::Wifi => "WiFi",
        };
        let state = match d.state {
            DeviceState::Online => "●",
            DeviceState::Offline => "✗",
            DeviceState::Unauthorized => "?",
            DeviceState::Connecting => "…",
        };
        println!(
            "{} {:<22} {:<22} {:<7} {:<16} {:<8} {:<8}",
            state,
            d.name,
            d.serial,
            conn,
            d.ip.clone().unwrap_or_default(),
            d.android_version.clone().unwrap_or_default(),
            d.battery.map(|b| format!("{b}%")).unwrap_or_default(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fl_adb::{CommandOutput, MockRunner};

    #[tokio::test]
    async fn enrich_adds_version_and_battery() {
        let r = MockRunner::new();
        r.expect("adb -s ABC shell getprop ro.build.version.release", CommandOutput::ok("14\n"));
        r.expect("adb -s ABC shell dumpsys battery", CommandOutput::ok("Current Battery Service state:\n  level: 87\n"));
        let mut devices = vec![Device {
            serial: "ABC".into(), name: "ABC".into(), model: None,
            connection: ConnectionKind::Usb, state: DeviceState::Online,
            ip: None, android_version: None, battery: None,
        }];
        enrich(&r, &mut devices).await;
        assert_eq!(devices[0].android_version.as_deref(), Some("14"));
        assert_eq!(devices[0].battery, Some(87));
    }
}
