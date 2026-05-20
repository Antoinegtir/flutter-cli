//! Dart VM Service JSON-RPC 2.0 envelope types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct Request<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'a str,
    pub params: Value,
}

impl<'a> Request<'a> {
    pub fn new(id: u64, method: &'a str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method,
            params,
        }
    }
    pub fn to_text(&self) -> String {
        serde_json::to_string(self).expect("Request serializes")
    }
}

#[derive(Debug, Deserialize)]
pub struct Response {
    pub id: Option<u64>,
    pub result: Option<Value>,
    pub error: Option<RpcError>,
    pub method: Option<String>,
    pub params: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug)]
pub enum Incoming {
    /// Result for a previously sent request.
    Reply {
        id: u64,
        result: Result<Value, RpcError>,
    },
    /// Stream event: `streamNotify` push.
    StreamEvent { stream_id: String, event: Value },
    /// Anything else we don't model.
    Other,
}

pub fn parse_incoming(text: &str) -> anyhow::Result<Incoming> {
    let r: Response = serde_json::from_str(text)?;
    if let Some(id) = r.id {
        let result = match (r.result, r.error) {
            (Some(v), _) => Ok(v),
            (_, Some(e)) => Err(e),
            _ => Err(RpcError {
                code: -32603,
                message: "empty reply".into(),
            }),
        };
        return Ok(Incoming::Reply { id, result });
    }
    if r.method.as_deref() == Some("streamNotify") {
        if let Some(p) = r.params {
            let stream_id = p
                .get("streamId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let event = p.get("event").cloned().unwrap_or(Value::Null);
            return Ok(Incoming::StreamEvent { stream_id, event });
        }
    }
    Ok(Incoming::Other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_serializes_with_jsonrpc_field() {
        let r = Request::new(1, "streamListen", json!({"streamId":"Stdout"})).to_text();
        assert!(r.contains(r#""jsonrpc":"2.0""#));
        assert!(r.contains(r#""id":1"#));
        assert!(r.contains(r#""method":"streamListen""#));
    }

    #[test]
    fn parse_reply_with_result() {
        let s = r#"{"jsonrpc":"2.0","id":7,"result":{"type":"Success"}}"#;
        match parse_incoming(s).unwrap() {
            Incoming::Reply { id, result } => {
                assert_eq!(id, 7);
                assert!(result.is_ok());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_reply_with_error() {
        let s = r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"nope"}}"#;
        match parse_incoming(s).unwrap() {
            Incoming::Reply { id, result } => {
                assert_eq!(id, 2);
                let e = result.unwrap_err();
                assert_eq!(e.code, -32601);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_stream_notify() {
        let s = r#"{"jsonrpc":"2.0","method":"streamNotify","params":{"streamId":"Stdout","event":{"bytes":"aGVsbG8="}}}"#;
        match parse_incoming(s).unwrap() {
            Incoming::StreamEvent { stream_id, event } => {
                assert_eq!(stream_id, "Stdout");
                assert_eq!(event["bytes"], "aGVsbG8=");
            }
            _ => panic!(),
        }
    }
}
