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
    // Apple device discovery uses `xcrun`, which is macOS-only — gate
    // it out on Linux/Windows to avoid two ENOENT spawns per `fl devices`
    // invocation. `cfg!` is const-folded so the branch is dropped at
    // compile time on non-macOS targets.
    if cfg!(target_os = "macos") {
        let xcrun = Xcrun::new(TokioRunner);
        devices.extend(fl_ios::list_apple_devices(&xcrun).await);
    }
    enrich(&runner, &mut devices).await;
    print_table(&devices);
    Ok(())
}

/// Fill in the Android OS version for adb-attached devices. iOS already
/// has its version populated by `xcrun devicectl` upstream. We no longer
/// fetch battery — the only reliable iOS source (libimobiledevice) isn't
/// installed by default and Apple's `devicectl` doesn't expose it.
async fn enrich<R: CommandRunner + ?Sized>(runner: &R, devices: &mut [Device]) {
    for d in devices.iter_mut() {
        let is_apple = d
            .platform
            .as_deref()
            .map(|p| {
                let p = p.to_ascii_lowercase();
                p.starts_with("ios")
                    || p.starts_with("ipad")
                    || p.starts_with("watch")
                    || p.contains("darwin")
                    || p.contains("macos")
            })
            .unwrap_or(false);
        if is_apple {
            continue;
        }
        if let Ok(o) = runner
            .run(
                "adb",
                &[
                    "-s",
                    &d.serial,
                    "shell",
                    "getprop",
                    "ro.build.version.release",
                ],
            )
            .await
        {
            let v = o.stdout.trim();
            if !v.is_empty() {
                d.android_version = Some(v.into());
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
        "  {:<24} {:<32} {:<10} {:<6} {:<7}",
        "NAME", "SERIAL", "PLATFORM", "CONN", "OS"
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
        let platform_raw = d.platform.clone().unwrap_or_else(|| "-".into());
        let plat_label = if platform_raw == "ios-simulator" {
            "ios-sim".to_string()
        } else {
            platform_raw.clone()
        };
        // Prefix with the same emoji used in the TUI panels so the user
        // gets visual parity between `fl devices` and the running dashboard.
        let plat_glyph = fl_tui::panels::devices::platform_icon(&platform_raw);
        // Emoji glyphs count as 1 char but render at 2 cols, so the emoji
        // branch uses a 7-char label pad (1+1+7 = 9 chars / 10 display
        // cols); the no-glyph branch pads to 10 chars for the same width.
        let plat_display = if plat_glyph.is_empty() {
            format!("{plat_label:<10}")
        } else {
            format!("{plat_glyph} {plat_label:<7}")
        };
        println!(
            "{} {:<24} {:<32} {} {:<6} {:<7}",
            state,
            truncate(&d.name, 24),
            truncate(&d.serial, 32),
            plat_display,
            conn,
            d.android_version.clone().unwrap_or_default(),
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
    async fn enrich_adds_android_version() {
        let r = MockRunner::new();
        r.expect(
            "adb -s ABC shell getprop ro.build.version.release",
            CommandOutput::ok("14\n"),
        );
        let mut devices = vec![Device {
            serial: "ABC".into(),
            name: "ABC".into(),
            model: None,
            connection: ConnectionKind::Usb,
            state: DeviceState::Online,
            ip: None,
            android_version: None,
            battery: None,
            platform: None,
        }];
        enrich(&r, &mut devices).await;
        assert_eq!(devices[0].android_version.as_deref(), Some("14"));
    }

    #[tokio::test]
    async fn enrich_skips_adb_calls_for_apple_devices() {
        // No `adb` expectations registered — MockRunner would panic on
        // unexpected calls, so this test asserts we don't shell out to
        // adb for iOS rows.
        let r = MockRunner::new();
        let mut devices = vec![Device {
            serial: "00008140-IPHONE".into(),
            name: "iPhone".into(),
            model: None,
            connection: ConnectionKind::Usb,
            state: DeviceState::Online,
            ip: None,
            android_version: Some("17.4".into()),
            battery: None,
            platform: Some("ios".into()),
        }];
        enrich(&r, &mut devices).await;
        assert_eq!(devices[0].android_version.as_deref(), Some("17.4"));
    }
}
