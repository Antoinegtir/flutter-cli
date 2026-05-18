//! Wraps the Flutter SDK CLI (`flutter --machine` daemon).

pub mod daemon;
pub mod parse;
pub mod path;
pub mod test_parse;

pub use daemon::FlutterDaemon;
pub use parse::parse_daemon_line;
pub use path::resolve_flutter;
pub use test_parse::parse_test_line;
