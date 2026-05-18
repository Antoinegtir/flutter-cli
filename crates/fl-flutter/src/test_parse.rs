//! Parser for `flutter test --machine` JSON lines.
//!
//! Each line is a plain JSON object (NOT wrapped in `[…]` like the daemon protocol).
//! Recognised `type` values: `start`, `suite`, `testStart`, `testDone`, `error`, `done`.

use fl_core::{TestEvent, TestResult};
use serde_json::Value;

pub fn parse_test_line(raw: &str) -> Option<TestEvent> {
    let raw = raw.trim();
    if !raw.starts_with('{') {
        return None;
    }
    let v: Value = serde_json::from_str(raw).ok()?;
    let kind = v.get("type")?.as_str()?;
    match kind {
        "suite" => {
            let suite = v.get("suite")?;
            let path = suite.get("path").and_then(Value::as_str)?.to_string();
            Some(TestEvent::SuiteStart { path })
        }
        "testStart" => {
            let t = v.get("test")?;
            let id = t.get("id").and_then(Value::as_u64)?;
            let name = t.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            Some(TestEvent::TestStarted { id, name })
        }
        "testDone" => {
            let id = v.get("testID").and_then(Value::as_u64)?;
            let name = v.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let result_s = v.get("result").and_then(Value::as_str).unwrap_or("");
            let result = match result_s {
                "success" => TestResult::Success,
                "failure" => TestResult::Failure,
                "error" => TestResult::Error,
                _ => TestResult::Skipped,
            };
            let duration_ms = v.get("time").and_then(Value::as_u64).unwrap_or(0);
            Some(TestEvent::TestDone { id, name, result, duration_ms })
        }
        "error" => {
            let id = v.get("testID").and_then(Value::as_u64);
            let message = v.get("error").and_then(Value::as_str).unwrap_or("").to_string();
            let stack = v.get("stackTrace").and_then(Value::as_str).map(str::to_string);
            Some(TestEvent::Error { id, message, stack })
        }
        "done" => {
            let success = v.get("success").and_then(Value::as_bool).unwrap_or(false);
            Some(TestEvent::AllDone { success, passed: 0, failed: 0, skipped: 0 })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_suite_start() {
        let line = r#"{"type":"suite","suite":{"id":1,"path":"test/widget_test.dart"}}"#;
        match parse_test_line(line).unwrap() {
            TestEvent::SuiteStart { path } => assert_eq!(path, "test/widget_test.dart"),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_test_start() {
        let line = r#"{"type":"testStart","time":12,"test":{"id":1,"name":"loads home"}}"#;
        match parse_test_line(line).unwrap() {
            TestEvent::TestStarted { id, name } => {
                assert_eq!(id, 1);
                assert_eq!(name, "loads home");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_test_done_success() {
        let line = r#"{"type":"testDone","testID":1,"result":"success","time":42,"name":"x"}"#;
        match parse_test_line(line).unwrap() {
            TestEvent::TestDone { id, result, .. } => {
                assert_eq!(id, 1);
                assert!(matches!(result, TestResult::Success));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_test_done_failure() {
        let line = r#"{"type":"testDone","testID":2,"result":"failure","time":100,"name":"y"}"#;
        match parse_test_line(line).unwrap() {
            TestEvent::TestDone { result, .. } => assert!(matches!(result, TestResult::Failure)),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_error_with_stack() {
        let line = r#"{"type":"error","testID":2,"error":"Expected X","stackTrace":"at line 5"}"#;
        match parse_test_line(line).unwrap() {
            TestEvent::Error { id, message, stack } => {
                assert_eq!(id, Some(2));
                assert!(message.contains("Expected"));
                assert!(stack.unwrap().contains("line 5"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_done_success() {
        let line = r#"{"type":"done","success":true,"time":2000}"#;
        match parse_test_line(line).unwrap() {
            TestEvent::AllDone { success, .. } => assert!(success),
            _ => panic!(),
        }
    }

    #[test]
    fn ignores_garbage() {
        assert!(parse_test_line("not json").is_none());
        assert!(parse_test_line(r#"{"type":"unknown"}"#).is_none());
    }
}
