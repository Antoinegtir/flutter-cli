//! Dart VM Service client.

pub mod client;
pub mod rpc;

pub use client::VmServiceClient;
pub use rpc::{parse_incoming, Incoming, Request, Response, RpcError};
