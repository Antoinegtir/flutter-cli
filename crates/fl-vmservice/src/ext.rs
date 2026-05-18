//! Convenience wrappers around Flutter's VM Service extensions.

use crate::client::VmServiceClient;
use serde_json::{json, Value};

impl VmServiceClient {
    pub async fn hot_reload(&self, isolate_id: &str) -> anyhow::Result<Value> {
        self.call("reloadSources", json!({ "isolateId": isolate_id })).await
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
