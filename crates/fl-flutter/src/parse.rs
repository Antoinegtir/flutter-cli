//! Parse one line of Flutter daemon `--machine` output into a `FlutterEvent`.

use fl_core::{FlutterEvent, LogLevel};
use serde_json::Value;

pub fn parse_daemon_line(raw: &str) -> Option<FlutterEvent> {
    let raw = raw.trim();
    if !raw.starts_with('[') {
        return None;
    }
    let v: Value = serde_json::from_str(raw).ok()?;
    let first = v.as_array()?.first()?;
    let obj = first.as_object()?;

    if let Some(event) = obj.get("event").and_then(Value::as_str) {
        return match event {
            "daemon.connected" => Some(FlutterEvent::DaemonReady),
            "app.started" => {
                // `app.started` doesn't always carry the VM URI — Flutter emits
                // it separately as `app.debugPort` earlier in the boot. Accept
                // either field, falling back to empty so we still surface the
                // event (and freeze the build chronometer).
                let params = obj.get("params")?;
                let app_id = params
                    .get("appId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let uri = params
                    .get("vmServiceUri")
                    .or_else(|| params.get("wsUri"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                Some(FlutterEvent::AppStarted {
                    app_id,
                    vm_service_uri: uri,
                })
            }
            // Only `app.stopped` is terminal. `app.stop` is an intermediate
            // shutdown-initiated event that fires mid-build for things like
            // pod install / Gradle hot-restart; treating it as terminal makes
            // fl exit prematurely.
            "app.stopped" => {
                let code = obj
                    .get("params")
                    .and_then(|p| p.get("exitCode"))
                    .and_then(Value::as_i64)
                    .map(|i| i as i32);
                Some(FlutterEvent::Stopped { exit_code: code })
            }
            "app.start" => {
                // app.start fires when the build begins. Surface it as an info
                // log so the user knows things are happening; not terminal.
                let params = obj.get("params");
                let name = params
                    .and_then(|p| p.get("appId"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Some(FlutterEvent::Log {
                    level: LogLevel::Info,
                    message: format!("build started ({name})"),
                })
            }
            // Device-list maintenance from the daemon — noisy and verbose
            // (each event carries the full device JSON). We already track
            // devices ourselves via ADB and xcrun, so drop them entirely.
            "device.added" | "device.removed" | "device.changed" => None,
            "app.debugPort" => {
                // VM Service WebSocket URL — promote to AppStarted so consumers
                // (chronometer freeze, VM Service connection) trigger on it.
                let params = obj.get("params")?;
                let app_id = params
                    .get("appId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let uri = params
                    .get("wsUri")
                    .or_else(|| params.get("uri"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                Some(FlutterEvent::AppStarted {
                    app_id,
                    vm_service_uri: uri,
                })
            }
            "app.devTools" | "app.dtd" => {
                // Connection-info side channels; summarise.
                let params = obj.get("params");
                let uri = params
                    .and_then(|p| p.get("wsUri").or_else(|| p.get("uri")))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Some(FlutterEvent::Log {
                    level: LogLevel::Debug,
                    message: format!("{event}: {uri}"),
                })
            }
            "daemon.logMessage" => {
                let params = obj.get("params")?;
                let level_s = params
                    .get("level")
                    .and_then(Value::as_str)
                    .unwrap_or("info");
                let msg = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                Some(FlutterEvent::Log {
                    level: parse_level(level_s),
                    message: msg,
                })
            }
            "app.log" => {
                let params = obj.get("params")?;
                let msg = params
                    .get("log")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let level = if params
                    .get("error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    LogLevel::Error
                } else {
                    LogLevel::Info
                };
                Some(FlutterEvent::Log {
                    level,
                    message: msg,
                })
            }
            "daemon.showMessage" => {
                let params = obj.get("params")?;
                let level_s = params
                    .get("level")
                    .and_then(Value::as_str)
                    .unwrap_or("info");
                let title = params.get("title").and_then(Value::as_str).unwrap_or("");
                let msg = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let combined = if title.is_empty() {
                    msg
                } else {
                    format!("{title}: {msg}")
                };
                Some(FlutterEvent::Log {
                    level: parse_level(level_s),
                    message: combined,
                })
            }
            "app.progress" => {
                let params = obj.get("params")?;
                let id = params
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let message = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let finished = params
                    .get("finished")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Some(FlutterEvent::Progress {
                    id,
                    message,
                    finished,
                })
            }
            _ => None,
        };
    }
    None
}

fn parse_level(s: &str) -> LogLevel {
    match s.to_ascii_lowercase().as_str() {
        "trace" => LogLevel::Trace,
        "debug" => LogLevel::Debug,
        "warn" | "warning" => LogLevel::Warn,
        "error" | "severe" => LogLevel::Error,
        _ => LogLevel::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_daemon_connected() {
        let line = r#"[{"event":"daemon.connected","params":{"version":"0.6.1","pid":123}}]"#;
        assert!(matches!(
            parse_daemon_line(line),
            Some(FlutterEvent::DaemonReady)
        ));
    }

    #[test]
    fn parses_app_started_with_vm_service_uri() {
        let line = r#"[{"event":"app.started","params":{"appId":"abc","vmServiceUri":"ws://127.0.0.1:55321/abc/ws"}}]"#;
        match parse_daemon_line(line).unwrap() {
            FlutterEvent::AppStarted {
                app_id,
                vm_service_uri,
            } => {
                assert_eq!(app_id, "abc");
                assert!(vm_service_uri.starts_with("ws://"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_daemon_log_warning_level() {
        let line = r#"[{"event":"daemon.logMessage","params":{"level":"warning","message":"slow build"}}]"#;
        match parse_daemon_line(line).unwrap() {
            FlutterEvent::Log { level, message } => {
                assert_eq!(level, LogLevel::Warn);
                assert_eq!(message, "slow build");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_app_log_error_flag() {
        let line = r#"[{"event":"app.log","params":{"appId":"x","log":"boom","error":true}}]"#;
        match parse_daemon_line(line).unwrap() {
            FlutterEvent::Log { level, message } => {
                assert_eq!(level, LogLevel::Error);
                assert_eq!(message, "boom");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn returns_none_on_garbage() {
        assert!(parse_daemon_line("not json").is_none());
        assert!(parse_daemon_line("").is_none());
        assert!(parse_daemon_line("[{}]").is_none());
    }
}
