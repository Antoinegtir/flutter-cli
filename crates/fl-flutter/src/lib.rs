//! Wraps the Flutter SDK CLI (`flutter --machine` daemon).

pub mod daemon;
pub mod doctor_parse;
pub mod parse;
pub mod path;
pub mod test_parse;

pub use daemon::FlutterDaemon;
pub use doctor_parse::parse_doctor_output;
pub use parse::parse_daemon_line;
pub use path::{resolve_flutter, sdk_versions, SdkVersions};
pub use test_parse::parse_test_line;
