//! Watching files through the library: the `omw::watch` primitive and the
//! script-to-agent mapping hot reload is built on.
//!
//! `Scripts` turns a set of agents into a watcher that reports which agents to
//! restart when a brain script changes; `Watcher` is the general primitive for
//! watching any path.

use std::time::Duration;

use omw::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let dir = tempfile::tempdir()?;
  let brain = dir.path().join("brain.rhai");
  std::fs::write(&brain, "// v1")?;

  // `Scripts` maps each script to the agents running it.
  let agents = vec![AgentConfig {
    name: "alice".to_string(),
    runtime: "rhai".to_string(),
    script: brain.to_string_lossy().into_owned(),
  }];
  let mut scripts = Scripts::with_tunables(&agents, Tunables::default())?;
  anyhow::ensure!(!scripts.is_empty(), "the script should be watchable");

  std::fs::write(&brain, "// v2")?;
  let reload =
    tokio::time::timeout(Duration::from_secs(10), scripts.next_reload())
      .await
      .map_err(|_| anyhow::anyhow!("timed out waiting for a reload"))?;
  anyhow::ensure!(
    reload == Some(vec!["alice".to_string()]),
    "unexpected reload: {reload:?}"
  );

  // The lower-level `Watcher` watches any path and yields changed paths.
  let mut watcher = Watcher::watch(
    &scope(dir.path()),
    RecursiveMode::Recursive,
    Duration::from_millis(50),
  )?;
  std::fs::write(dir.path().join("other.txt"), "hi")?;
  let paths =
    tokio::time::timeout(Duration::from_secs(10), watcher.next_change())
      .await
      .map_err(|_| anyhow::anyhow!("timed out waiting for a change"))?
      .ok_or_else(|| anyhow::anyhow!("watcher closed"))?;
  anyhow::ensure!(
    paths.iter().any(|path| path.ends_with("other.txt")),
    "unexpected changed paths: {paths:?}"
  );

  println!("watched a reload and {} changed path(s)", paths.len());
  Ok(())
}
