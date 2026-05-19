//! Dart VM Service client.

pub mod client;
pub mod ext;
pub mod mdns;
pub mod rpc;

#[cfg(test)]
pub mod tests_support;

pub use client::VmServiceClient;
pub use rpc::{parse_incoming, Incoming, Request, Response, RpcError};
