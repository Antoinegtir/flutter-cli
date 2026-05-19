//! Convenience wrappers around Flutter's VM Service extensions.

use crate::client::{decode_b64_bytes, VmServiceClient};
use serde_json::{json, Value};

impl VmServiceClient {
    pub async fn hot_reload(&self, isolate_id: &str) -> anyhow::Result<Value> {
        self.call("reloadSources", json!({ "isolateId": isolate_id })).await
    }

    /// Capture the current frame as a PNG via Flutter's
    /// `_flutter.screenshot` VM Service RPC. Works on every platform
    /// Flutter supports the moment a VM Service is connected — no
    /// `adb` / `libimobiledevice` / Xcode tooling required. This is
    /// the same RPC DevTools' screenshot button calls.
    pub async fn screenshot_png(&self) -> anyhow::Result<Vec<u8>> {
        let v = self.call("_flutter.screenshot", json!({})).await?;
        let b64 = v
            .get("screenshot")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("screenshot RPC returned no `screenshot` field"))?;
        let bytes = decode_b64_bytes(b64);
        if !bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            return Err(anyhow::anyhow!(
                "screenshot RPC returned {} bytes that don't look like a PNG",
                bytes.len()
            ));
        }
        Ok(bytes)
    }

    pub async fn hot_restart(&self, isolate_id: &str) -> anyhow::Result<Value> {
        self.call(
            "callServiceExtension",
            json!({ "isolateId": isolate_id, "method": "s0.hotRestart" }),
        )
        .await
    }

    pub async fn toggle_brightness(&self, isolate_id: &str, dark: bool) -> anyhow::Result<Value> {
        let value = if dark { "Brightness.dark" } else { "Brightness.light" };
        self.call(
            "ext.flutter.brightnessOverride",
            json!({ "isolateId": isolate_id, "value": value }),
        )
        .await
    }

    /// Set the Flutter framework's brightness override to a specific state,
    /// or `None` to clear the override and follow the host system again.
    /// Mirrors `flutter run`'s `b` key cycle (system → light → dark → system).
    pub async fn set_brightness(&self, isolate_id: &str, value: Option<bool>) -> anyhow::Result<Value> {
        let v = match value {
            Some(true) => "Brightness.dark",
            Some(false) => "Brightness.light",
            None => "default",
        };
        self.call(
            "ext.flutter.brightnessOverride",
            json!({ "isolateId": isolate_id, "value": v }),
        )
        .await
    }

    pub async fn toggle_debug_paint(&self, isolate_id: &str, enabled: bool) -> anyhow::Result<Value> {
        self.call(
            "ext.flutter.debugPaint",
            json!({ "isolateId": isolate_id, "enabled": enabled }),
        )
        .await
    }

    pub async fn toggle_platform(&self, isolate_id: &str, ios: bool) -> anyhow::Result<Value> {
        let value = if ios { "iOS" } else { "android" };
        self.call(
            "ext.flutter.platformOverride",
            json!({ "isolateId": isolate_id, "value": value }),
        )
        .await
    }

    pub async fn toggle_performance_overlay(&self, isolate_id: &str, enabled: bool) -> anyhow::Result<Value> {
        self.call(
            "ext.flutter.showPerformanceOverlay",
            json!({ "isolateId": isolate_id, "enabled": enabled }),
        )
        .await
    }

    /// Snapshot the isolate's heap usage. Returns `(used_mb, capacity_mb)`.
    /// We poll this periodically because the VM Service doesn't push memory
    /// stats — `streamListen("GC")` only emits GC events, not totals.
    ///
    /// The Dart VM Service exposes this under `getMemoryUsage` (current,
    /// public). Older builds shipped it as `getIsolateMemoryUsage` for a
    /// while; we try the modern name first and fall back to the legacy
    /// one if the VM responds with `-32601 Unknown method`, so the panel
    /// works across the entire range of Flutter SDKs the user might be
    /// running.
    pub async fn get_memory_usage_mb(&self, isolate_id: &str) -> anyhow::Result<(f64, f64)> {
        let args = json!({ "isolateId": isolate_id });
        let v = match self.call("getMemoryUsage", args.clone()).await {
            Ok(v) => v,
            Err(e) => {
                let msg = e.to_string();
                // -32601 = "Method not found". Only fall back in that
                // specific case — any other error (timeout, transport,
                // sentinel) should surface as-is so the caller's log
                // path can show something actionable.
                if msg.contains("-32601") {
                    self.call("getIsolateMemoryUsage", args).await?
                } else {
                    return Err(e);
                }
            }
        };
        // VM service returns bytes for heapUsage / externalUsage / heapCapacity.
        let heap_used = v.get("heapUsage").and_then(Value::as_f64).unwrap_or(0.0);
        let external = v.get("externalUsage").and_then(Value::as_f64).unwrap_or(0.0);
        let capacity = v.get("heapCapacity").and_then(Value::as_f64).unwrap_or(0.0);
        let mb = 1024.0 * 1024.0;
        Ok(((heap_used + external) / mb, capacity / mb))
    }

    pub async fn get_first_isolate_id(&self) -> anyhow::Result<String> {
        let vm = self.call("getVM", json!({})).await?;
        let isolates = vm.get("isolates").and_then(Value::as_array).cloned().unwrap_or_default();
        let id = isolates
            .first()
            .and_then(|i| i.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("no isolates"))?;
        Ok(id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_support::spawn_mock_handler;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn get_first_isolate_id_returns_first() {
        let uri = spawn_mock_handler(|req| {
            assert_eq!(req["method"], "getVM");
            json!({"isolates":[{"id":"isolates/1"},{"id":"isolates/2"}]})
        }).await;
        let (tx, _rx) = mpsc::channel(8);
        let client = VmServiceClient::connect(&uri, tx).await.unwrap();
        let id = client.get_first_isolate_id().await.unwrap();
        assert_eq!(id, "isolates/1");
    }

    #[tokio::test]
    async fn hot_reload_calls_reload_sources() {
        let uri = spawn_mock_handler(|req| {
            assert_eq!(req["method"], "reloadSources");
            assert_eq!(req["params"]["isolateId"], "isolates/1");
            json!({"type":"Success"})
        }).await;
        let (tx, _rx) = mpsc::channel(8);
        let client = VmServiceClient::connect(&uri, tx).await.unwrap();
        let v = client.hot_reload("isolates/1").await.unwrap();
        assert_eq!(v["type"], "Success");
    }
}
