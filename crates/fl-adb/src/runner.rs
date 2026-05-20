//! `CommandRunner` abstracts process execution so tests can inject fake outputs.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub status: i32,
}

impl CommandOutput {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            stderr: String::new(),
            status: 0,
        }
    }
}

#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, program: &str, args: &[&str]) -> anyhow::Result<CommandOutput>;
}

/// Production runner that spawns real processes via tokio.
#[derive(Default)]
pub struct TokioRunner;

#[async_trait]
impl CommandRunner for TokioRunner {
    async fn run(&self, program: &str, args: &[&str]) -> anyhow::Result<CommandOutput> {
        let output = tokio::process::Command::new(program)
            .args(args)
            .output()
            .await?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status.code().unwrap_or(-1),
        })
    }
}

/// Test runner that returns pre-canned outputs keyed by `"program arg1 arg2 ..."`.
#[derive(Default)]
pub struct MockRunner {
    responses: Mutex<HashMap<String, CommandOutput>>,
    calls: Mutex<Vec<String>>,
}

impl MockRunner {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn expect(&self, cmd: &str, out: CommandOutput) {
        self.responses.lock().unwrap().insert(cmd.into(), out);
    }
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl CommandRunner for MockRunner {
    async fn run(&self, program: &str, args: &[&str]) -> anyhow::Result<CommandOutput> {
        let key = std::iter::once(program)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        self.calls.lock().unwrap().push(key.clone());
        self.responses
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("MockRunner: no canned response for `{key}`"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_records_calls_and_returns_canned_output() {
        let m = MockRunner::new();
        m.expect(
            "adb devices -l",
            CommandOutput::ok("List of devices attached\n"),
        );
        let out = m.run("adb", &["devices", "-l"]).await.unwrap();
        assert_eq!(out.status, 0);
        assert!(out.stdout.contains("List of devices"));
        assert_eq!(m.calls(), vec!["adb devices -l".to_string()]);
    }

    #[tokio::test]
    async fn mock_errors_on_unexpected_call() {
        let m = MockRunner::new();
        let err = m.run("adb", &["devices"]).await.unwrap_err();
        assert!(err.to_string().contains("no canned response"));
    }
}
