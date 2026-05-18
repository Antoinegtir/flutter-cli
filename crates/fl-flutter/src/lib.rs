//! Wraps the Flutter SDK CLI (`flutter --machine` daemon).

pub mod parse;
pub mod path;

pub use parse::parse_daemon_line;
pub use path::resolve_flutter;
