//! Dart VM Service client.

pub mod rpc;

pub use rpc::{parse_incoming, Incoming, Request, Response, RpcError};
