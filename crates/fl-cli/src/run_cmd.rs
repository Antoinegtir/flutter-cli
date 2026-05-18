use std::path::PathBuf;

pub async fn run(_project: Option<PathBuf>, _device: Option<String>, _no_wifi: bool) -> anyhow::Result<()> {
    println!("fl run (stub)");
    Ok(())
}
