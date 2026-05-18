//! Helpers shared by tests of multiple modules in this crate.
#![cfg(test)]

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

pub async fn spawn_mock_handler<F>(handler: F) -> String
where
    F: Fn(Value) -> Value + Send + Sync + 'static,
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
                        let req: Value = serde_json::from_str(&t).unwrap();
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
