//! Debounced filesystem watching.
//!
//! [`Watcher`] is the general primitive: point it at one or more paths with a
//! debounce window and await the next batch of changed paths. [`Scripts`]
//! builds on it to map each agent's brain script to the agents running it, so
//! a supervisor can restart just the affected agents on change.
//!
//! Parent directories are watched (non-recursively) by [`Scripts`] so editors
//! that save atomically (`write temp + rename`) still trigger, and events are
//! debounced so a single save restarts an agent once. Only create/modify/remove
//! (and rescan) events count as changes; access/read/open/close events are
//! ignored, so a watcher does not react to its own directory reads. State
//! preservation is the supervisor's job: only agent names are reported here;
//! the supervisor keeps the shared
//! [`MessageBus`](crate::host::bus::MessageBus) (inboxes, subscriptions) alive
//! and restarts just the run.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use notify::event::EventKind;
use notify_debouncer_full::notify::RecommendedWatcher;
use notify_debouncer_full::{
  DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache,
  new_debouncer,
};
use tokio::sync::mpsc;

use crate::config::{AgentConfig, Tunables};

pub use notify_debouncer_full::notify::RecursiveMode;

/// Whether an event is a real change (not a read/open/close).
fn is_change(event: &DebouncedEvent) -> bool {
  !matches!(event.kind, EventKind::Access(_))
}

/// A debounced watcher over zero or more paths.
pub struct Watcher {
  _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
  rx: mpsc::UnboundedReceiver<Vec<PathBuf>>,
}

impl Watcher {
  /// Create a watcher with no paths yet; add them with [`add`](Self::add).
  pub fn new(debounce: Duration) -> anyhow::Result<Self> {
    let (tx, rx) = mpsc::unbounded_channel();
    let debouncer =
      new_debouncer(debounce, None, move |result: DebounceEventResult| {
        match result {
          Ok(events) => {
            let paths = events
              .iter()
              .filter(|event| is_change(event))
              .flat_map(|event| event.paths.iter().cloned())
              .collect::<Vec<_>>();
            if !paths.is_empty() {
              let _ = tx.send(paths);
            }
          }
          Err(errors) => {
            for error in errors {
              tracing::warn!(error = %error, "watcher error");
            }
          }
        }
      })
      .context("failed to create the watcher")?;
    Ok(Self {
      _debouncer: debouncer,
      rx,
    })
  }

  /// Create a watcher over `path` watched with `mode`.
  pub fn watch(
    path: &Path,
    mode: RecursiveMode,
    debounce: Duration,
  ) -> anyhow::Result<Self> {
    let mut watcher = Self::new(debounce)?;
    watcher.add(path, mode)?;
    Ok(watcher)
  }

  /// Add another `path` to this watcher.
  pub fn add(
    &mut self,
    path: &Path,
    mode: RecursiveMode,
  ) -> anyhow::Result<()> {
    self
      ._debouncer
      .watch(path, mode)
      .with_context(|| format!("failed to watch {}", path.display()))
  }

  /// Wait for the next debounced batch of changed paths. `None` means the
  /// watcher is gone.
  pub async fn next_change(&mut self) -> Option<Vec<PathBuf>> {
    self.rx.recv().await
  }
}

/// One watched script file plus every agent running it.
#[derive(Debug)]
struct WatchedScript {
  /// Canonical path when the file exists, else the absolute path.
  path: PathBuf,
  /// Canonical parent directory (always exists; it is what is watched).
  parent: PathBuf,
  file_name: OsString,
  agents: Vec<String>,
}

/// Watches every agent script in `agents`, reporting agent names to restart.
pub struct Scripts {
  watcher: Watcher,
  scripts: Vec<WatchedScript>,
}

impl Scripts {
  /// Start watching the scripts of `agents`. Scripts whose parent directory
  /// does not exist are skipped with a warning.
  #[cfg(test)]
  pub fn new(agents: &BTreeMap<String, AgentConfig>) -> anyhow::Result<Self> {
    Self::with_tunables(agents, Tunables::default())
  }

  /// [`new`](Self::new) with an explicit debounce.
  pub fn with_tunables(
    agents: &BTreeMap<String, AgentConfig>,
    tunables: Tunables,
  ) -> anyhow::Result<Self> {
    let mut scripts: Vec<WatchedScript> = Vec::new();
    for (name, agent) in agents {
      let script = absolute(&PathBuf::from(&agent.script))?;
      let Some(file_name) = script.file_name().map(OsString::from) else {
        tracing::warn!(
          agent = %name,
          script = %agent.script,
          "cannot watch a script without a file name"
        );
        continue;
      };
      let Some(parent) = script.parent().map(Path::to_path_buf) else {
        tracing::warn!(
          agent = %name,
          script = %agent.script,
          "cannot watch a script without a parent directory"
        );
        continue;
      };
      let parent = match parent.canonicalize() {
        Ok(parent) => parent,
        Err(error) => {
          tracing::warn!(
            agent = %name,
            script = %agent.script,
            error = %error,
            "cannot watch a script whose directory does not exist"
          );
          continue;
        }
      };
      let path = canonical_or_absolute(&script);
      if let Some(existing) = scripts.iter_mut().find(|script| {
        script.path == path
          || (script.parent == parent && script.file_name == file_name)
      }) {
        if !existing.agents.contains(name) {
          existing.agents.push(name.clone());
        }
        continue;
      }
      scripts.push(WatchedScript {
        path,
        parent,
        file_name,
        agents: vec![name.clone()],
      });
    }

    let mut watcher = Watcher::new(tunables.watch_debounce())?;
    let mut dirs: HashSet<PathBuf> = HashSet::new();
    for script in &scripts {
      if dirs.insert(script.parent.clone()) {
        watcher.add(&script.parent, RecursiveMode::NonRecursive)?;
      }
    }
    tracing::info!(
      scripts = scripts.len(),
      dirs = dirs.len(),
      "watching agent scripts for hot reload"
    );
    Ok(Self { watcher, scripts })
  }

  /// Whether no script could be watched.
  pub fn is_empty(&self) -> bool {
    self.scripts.is_empty()
  }

  /// Wait for the next script change, returning the affected agent names in
  /// sorted order. Events for unrelated files in watched directories are
  /// skipped internally; `None` means the watcher is gone.
  pub async fn next_reload(&mut self) -> Option<Vec<String>> {
    while let Some(paths) = self.watcher.next_change().await {
      let agents = resolve(&paths, &self.scripts);
      if agents.is_empty() {
        continue;
      }
      tracing::info!(agents = ?agents, "agent script changed");
      return Some(agents);
    }
    None
  }
}

/// Absolute form of `path`, resolved against the current directory.
fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
  if path.is_absolute() {
    Ok(path.to_path_buf())
  } else {
    let current = std::env::current_dir()
      .context("failed to read the current directory")?;
    Ok(current.join(path))
  }
}

/// Canonical form of `path`, falling back to the absolute path when the file
/// does not (yet) exist.
fn canonical_or_absolute(path: &Path) -> PathBuf {
  path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Agent names (sorted, deduped) whose watched script matches one of `paths`.
fn resolve(paths: &[PathBuf], scripts: &[WatchedScript]) -> Vec<String> {
  let mut agents: HashMap<String, ()> = HashMap::new();
  for path in paths {
    let canonical = canonical_or_absolute(path);
    let parent = path.parent().map(canonical_or_absolute);
    let file_name = path.file_name();
    for script in scripts {
      let hit = canonical == script.path
        || (file_name == Some(script.file_name.as_os_str())
          && parent.as_ref() == Some(&script.parent));
      if hit {
        for agent in &script.agents {
          agents.insert(agent.clone(), ());
        }
      }
    }
  }
  let mut agents: Vec<String> = agents.into_keys().collect();
  agents.sort();
  agents
}

#[cfg(test)]
mod tests {
  use super::*;

  fn agent(script: &str) -> AgentConfig {
    AgentConfig {
      runtime: "rhai".to_string(),
      script: script.to_string(),
    }
  }

  fn agents(entries: &[(&str, &str)]) -> BTreeMap<String, AgentConfig> {
    entries
      .iter()
      .map(|(name, script)| (name.to_string(), agent(script)))
      .collect()
  }

  fn watched(
    dir: &Path,
    name: &str,
    agents: &[&str],
  ) -> anyhow::Result<WatchedScript> {
    Ok(WatchedScript {
      path: dir.join(name).canonicalize()?,
      parent: dir.canonicalize()?,
      file_name: OsString::from(name),
      agents: agents.iter().map(ToString::to_string).collect(),
    })
  }

  #[test]
  fn resolve_matches_scripts_and_dedupes_agents() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let brain = dir.path().join("brain.rhai");
    let other = dir.path().join("other.rhai");
    std::fs::write(&brain, "1")?;
    std::fs::write(&other, "2")?;
    let scripts = vec![watched(dir.path(), "brain.rhai", &["alice", "bob"])?];

    assert_eq!(resolve(&[brain], &scripts), vec!["alice", "bob"]);
    assert!(resolve(&[other], &scripts).is_empty());
    assert!(resolve(&[], &scripts).is_empty());
    Ok(())
  }

  #[test]
  fn new_groups_agents_sharing_a_script() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let brain = dir.path().join("brain.rhai");
    std::fs::write(&brain, "1")?;
    let script = brain.to_string_lossy().to_string();
    let scripts =
      Scripts::new(&agents(&[("alice", &script), ("bob", &script)]))?;
    assert!(!scripts.is_empty());
    assert_eq!(scripts.scripts.len(), 1);
    assert_eq!(scripts.scripts[0].agents, vec!["alice", "bob"]);
    Ok(())
  }

  #[test]
  fn new_skips_scripts_without_a_watchable_directory() -> anyhow::Result<()> {
    let scripts =
      Scripts::new(&agents(&[("alice", "/nonexistent-dir-omw/brain.rhai")]))?;
    assert!(scripts.is_empty());
    Ok(())
  }

  #[tokio::test]
  async fn detects_a_script_modification() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let brain = dir.path().join("brain.rhai");
    std::fs::write(&brain, "v1")?;
    let mut scripts =
      Scripts::new(&agents(&[("alice", &brain.to_string_lossy())]))?;
    std::fs::write(&brain, "v2")?;
    let reload =
      tokio::time::timeout(Duration::from_secs(10), scripts.next_reload())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for a reload"))?
        .ok_or_else(|| anyhow::anyhow!("watcher closed"))?;
    assert_eq!(reload, vec!["alice".to_string()]);
    Ok(())
  }

  #[tokio::test]
  async fn ignores_unrelated_files_in_watched_directories() -> anyhow::Result<()>
  {
    let dir = tempfile::tempdir()?;
    let brain = dir.path().join("brain.rhai");
    std::fs::write(&brain, "v1")?;
    let mut scripts =
      Scripts::new(&agents(&[("alice", &brain.to_string_lossy())]))
        .map_err(|e| anyhow::anyhow!(e))?;
    std::fs::write(dir.path().join("other.rhai"), "unrelated")?;
    assert!(
      tokio::time::timeout(Duration::from_secs(2), scripts.next_reload())
        .await
        .is_err(),
      "an unrelated file should not trigger a reload"
    );
    Ok(())
  }

  #[tokio::test]
  async fn reading_a_watched_directory_is_not_a_change() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut watcher = Watcher::watch(
      dir.path(),
      RecursiveMode::Recursive,
      Duration::from_millis(50),
    )?;
    let entries: Vec<_> = std::fs::read_dir(dir.path())?.collect();
    drop(entries);
    assert!(
      tokio::time::timeout(Duration::from_secs(2), watcher.next_change())
        .await
        .is_err(),
      "reading a watched directory should not be a change"
    );
    Ok(())
  }

  #[tokio::test]
  async fn moving_a_directory_into_the_tree_reports_a_change()
  -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let source = outside.path().join("case");
    std::fs::create_dir(&source)?;
    std::fs::write(source.join("omw.test.toml"), "")?;
    let mut watcher = Watcher::watch(
      root.path(),
      RecursiveMode::Recursive,
      Duration::from_millis(50),
    )?;
    let moved = root.path().join("case");
    std::fs::rename(&source, &moved)?;
    let paths =
      tokio::time::timeout(Duration::from_secs(10), watcher.next_change())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for the move"))?
        .ok_or_else(|| anyhow::anyhow!("watcher closed"))?;
    assert!(
      paths.iter().any(|path| path.starts_with(root.path())),
      "the moved directory should be reported: {paths:?}"
    );
    Ok(())
  }
}
