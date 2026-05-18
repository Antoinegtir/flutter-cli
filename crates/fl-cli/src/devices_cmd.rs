//! `fl devices` — list attached devices using fl-adb + fl-ios.

use fl_adb::{parse_devices_l, CommandRunner, TokioRunner};
use fl_core::{ConnectionKind, Device, DeviceState};
use fl_ios::Xcrun;

pub async fn run() -> anyhow::Result<()> {
    let runner = TokioRunner;
    let mut devices: Vec<Device> = Vec::new();
    if let Ok(out) = runner.run("adb", &["devices", "-l"]).await {
        if out.status == 0 {
            devices.extend(parse_devices_l(&out.stdout));
        }
    }
    let xcrun = Xcrun::new(TokioRunner);
    devices.extend(fl_ios::list_apple_devices(&xcrun).await);
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
    println!(
        "  {:<24} {:<32} {:<8} {:<6} {:<7} {:<6}",
        "NAME", "SERIAL", "PLATFORM", "CONN", "OS", "BAT"
    );
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
        let platform = d.platform.clone().unwrap_or_else(|| "-".into());
        let plat_display = if platform == "ios-simulator" { "ios-sim".to_string() } else { platform };
        println!(
            "{} {:<24} {:<32} {:<8} {:<6} {:<7} {:<6}",
            state,
            truncate(&d.name, 24),
            truncate(&d.serial, 32),
            plat_display,
            conn,
            d.android_version.clone().unwrap_or_default(),
            d.battery.map(|b| format!("{b}%")).unwrap_or_default(),
        );
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max - 1).collect::<String>() + "…"
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
            ip: None, android_version: None, battery: None, platform: None,
        }];
        enrich(&r, &mut devices).await;
        assert_eq!(devices[0].android_version.as_deref(), Some("14"));
        assert_eq!(devices[0].battery, Some(87));
    }
}
