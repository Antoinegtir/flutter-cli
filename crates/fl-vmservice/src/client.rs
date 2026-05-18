//! Async WebSocket client for the Dart VM Service. Handles ID assignment,
//! pending-reply correlation, and stream subscription forwarding.

use crate::rpc::{parse_incoming, Incoming, Request, RpcError};
use anyhow::{anyhow, Context};
use fl_core::VmEvent;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::Message;

type Pending = HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>;

#[derive(Clone)]
pub struct VmServiceClient {
    next_id: Arc<AtomicU64>,
    out_tx: mpsc::Sender<Message>,
    pending: Arc<Mutex<Pending>>,
}

impl VmServiceClient {
    /// Connect to `uri`, spawn read/write loops, and emit decoded `VmEvent`s into `events_tx`.
    pub async fn connect(uri: &str, events_tx: mpsc::Sender<VmEvent>) -> anyhow::Result<Self> {
        let (ws_stream, _) = tokio_tungstenite::connect_async(uri)
            .await
            .context("connecting to VM Service")?;
        let (mut sink, mut stream) = ws_stream.split();

        let (out_tx, mut out_rx) = mpsc::channel::<Message>(64);
        let pending: Arc<Mutex<Pending>> = Arc::new(Mutex::new(HashMap::new()));
        let pending_w = pending.clone();
        let events_tx_w = events_tx.clone();

        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });

        tokio::spawn(async move {
            events_tx_w.send(VmEvent::Connected).await.ok();
            while let Some(msg) = stream.next().await {
                let txt = match msg {
                    Ok(Message::Text(t)) => t,
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => continue,
                };
                let parsed = match parse_incoming(&txt) {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                match parsed {
                    Incoming::Reply { id, result } => {
                        if let Some(tx) = pending_w.lock().await.remove(&id) {
                            let _ = tx.send(result);
                        }
                    }
                    Incoming::StreamEvent { stream_id, event } => {
                        if let Some(v) = stream_event_to_vm_event(&stream_id, &event) {
                            events_tx_w.send(v).await.ok();
                        }
                    }
                    Incoming::Other => {}
                }
            }
            events_tx_w.send(VmEvent::Disconnected).await.ok();
        });

        Ok(Self {
            next_id: Arc::new(AtomicU64::new(1)),
            out_tx,
            pending,
        })
    }

    pub async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = Request::new(id, method, params);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        self.out_tx
            .send(Message::Text(req.to_text()))
            .await
            .map_err(|_| anyhow!("ws writer closed"))?;
        match rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(anyhow!("rpc error {}: {}", e.code, e.message)),
            Err(_) => Err(anyhow!("reply channel dropped")),
        }
    }

    pub async fn stream_listen(&self, stream_id: &str) -> anyhow::Result<()> {
        self.call("streamListen", json!({ "streamId": stream_id })).await?;
        Ok(())
    }
}

fn stream_event_to_vm_event(stream_id: &str, event: &Value) -> Option<VmEvent> {
    let kind = event.get("kind").and_then(Value::as_str)?;
    match (stream_id, kind) {
        ("Stdout", "WriteEvent") => {
            let b64 = event.get("bytes").and_then(Value::as_str)?;
            let decoded = decode_b64_lossy(b64);
            Some(VmEvent::Stdout(decoded))
        }
        ("Stderr", "WriteEvent") => {
            let b64 = event.get("bytes").and_then(Value::as_str)?;
            Some(VmEvent::Stderr(decode_b64_lossy(b64)))
        }
        ("Isolate", _) => Some(VmEvent::IsolateEvent(kind.to_string())),
        ("Extension", "Extension") => {
            let name = event.get("extensionKind").and_then(Value::as_str).unwrap_or("");
            if name == "Flutter.FrameTiming" {
                let data = event.get("extensionData")?;
                let ui = data.get("ui").and_then(Value::as_u64).unwrap_or(0);
                let raster = data.get("raster").and_then(Value::as_u64).unwrap_or(0);
                Some(VmEvent::FrameTiming { ui_micros: ui, raster_micros: raster })
            } else {
                None
            }
        }
        ("GC", _) => {
            let new = event.get("new").and_then(|n| n.get("used")).and_then(Value::as_f64).unwrap_or(0.0);
            let old = event.get("old").and_then(|o| o.get("used")).and_then(Value::as_f64).unwrap_or(0.0);
            let cap_new = event.get("new").and_then(|n| n.get("capacity")).and_then(Value::as_f64).unwrap_or(0.0);
            let cap_old = event.get("old").and_then(|o| o.get("capacity")).and_then(Value::as_f64).unwrap_or(0.0);
            Some(VmEvent::GcStats {
                used_mb: (new + old) / (1024.0 * 1024.0),
                total_mb: (cap_new + cap_old) / (1024.0 * 1024.0),
            })
        }
        _ => None,
    }
}

fn decode_b64_lossy(input: &str) -> String {
    let table = |c: u8| -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let cleaned: Vec<u8> = input.bytes().filter(|&b| b != b'\n' && b != b'=').collect();
    let mut out = Vec::with_capacity(cleaned.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits = 0;
    for &b in &cleaned {
        if let Some(v) = table(b) {
            buf = (buf << 6) | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push(((buf >> bits) & 0xFF) as u8);
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    async fn spawn_mock_ws<F>(handler: F) -> String
    where
        F: Fn(serde_json::Value) -> serde_json::Value + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = Arc::new(handler);
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let handler = handler.clone();
                tokio::spawn(async move {
                    let mut ws = accept_async(sock).await.unwrap();
                    while let Some(Ok(msg)) = ws.next().await {
                        if let Message::Text(t) = msg {
                            let req: serde_json::Value = serde_json::from_str(&t).unwrap();
                            let id = req["id"].as_u64().unwrap_or(0);
                            let result = handler(req);
                            let reply = json!({"jsonrpc":"2.0","id":id,"result":result});
                            ws.send(Message::Text(reply.to_string())).await.ok();
                        }
                    }
                });
            }
        });
        format!("ws://{}", addr)
    }

    #[tokio::test]
    async fn call_returns_result_payload() {
        let uri = spawn_mock_ws(|_req| json!({"type":"Success"})).await;
        let (tx, mut _rx) = mpsc::channel(16);
        let client = VmServiceClient::connect(&uri, tx).await.unwrap();
        let v = client.call("streamListen", json!({"streamId":"Stdout"})).await.unwrap();
        assert_eq!(v["type"], "Success");
    }

    #[test]
    fn decode_base64_basic() {
        assert_eq!(decode_b64_lossy("aGVsbG8="), "hello");
        assert_eq!(decode_b64_lossy("aGVsbG8gd29ybGQ="), "hello world");
    }
}
