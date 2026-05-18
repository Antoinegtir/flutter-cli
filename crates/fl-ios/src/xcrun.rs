//! Thin wrapper around `xcrun` invocations via `CommandRunner`.

use fl_adb::CommandRunner;

pub struct Xcrun<R: CommandRunner> {
    runner: R,
}

impl<R: CommandRunner> Xcrun<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    pub async fn devicectl_list(&self) -> anyhow::Result<String> {
        let out = self
            .runner
            .run("xcrun", &["devicectl", "list", "devices", "--json-output", "-"])
            .await?;
        Ok(out.stdout)
    }

    pub async fn simctl_list(&self) -> anyhow::Result<String> {
        let out = self.runner.run("xcrun", &["simctl", "list", "devices", "--json"]).await?;
        Ok(out.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fl_adb::{CommandOutput, MockRunner};

    #[tokio::test]
    async fn devicectl_list_invokes_correct_command() {
        let m = MockRunner::new();
        m.expect("xcrun devicectl list devices --json-output -", CommandOutput::ok("{\"x\":1}"));
        let x = Xcrun::new(m);
        let out = x.devicectl_list().await.unwrap();
        assert_eq!(out, "{\"x\":1}");
    }

    #[tokio::test]
    async fn simctl_list_invokes_correct_command() {
        let m = MockRunner::new();
        m.expect("xcrun simctl list devices --json", CommandOutput::ok("{\"y\":2}"));
        let x = Xcrun::new(m);
        let out = x.simctl_list().await.unwrap();
        assert_eq!(out, "{\"y\":2}");
    }
}
