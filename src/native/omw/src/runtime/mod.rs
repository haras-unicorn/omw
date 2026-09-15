//! Runtime abstractions: how an agent brain is loaded and driven.
//!
//! A runtime instance is constructed from an impl-agnostic config entry
//! (a name plus opaque params) by the [`Registry`].

#[cfg(feature = "runtime-wasm")]
mod bindings;
#[cfg(feature = "runtime-wasm")]
mod engine;
#[cfg(feature = "runtime-wasm")]
mod host;
#[cfg(feature = "runtime-js")]
mod js;
#[cfg(feature = "runtime-rhai")]
mod rhai;
#[cfg(feature = "runtime-wasm")]
mod wasm;

use crate::host::ctx::AgentContext;
use serde_json::Value;
use std::sync::Arc;

/// The terminal result of one agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
  /// The brain ran to completion.
  Completed,
  /// The brain terminated itself with a message.
  Exited(String),
}

/// A configured runtime instance: the impl plus its config-derived name and
/// static kind.
#[derive(Clone)]
pub struct RuntimeEntry {
  name: String,
  kind: &'static str,
  inner: Arc<dyn Runtime>,
}

impl RuntimeEntry {
  /// Build an entry from a name and an implementation; the kind comes from
  /// the implementation itself.
  pub fn new<T: Runtime + 'static>(
    name: impl Into<String>,
    runtime: Arc<T>,
  ) -> Self {
    Self {
      name: name.into(),
      kind: T::kind(),
      inner: runtime,
    }
  }

  /// The config-derived name of this instance.
  pub fn name(&self) -> &str {
    &self.name
  }

  /// The static kind of this instance's implementation.
  pub fn kind(&self) -> &'static str {
    self.kind
  }

  /// The underlying implementation.
  pub fn inner(&self) -> &Arc<dyn Runtime> {
    &self.inner
  }
}

impl std::fmt::Debug for RuntimeEntry {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("RuntimeEntry")
      .field("name", &self.name)
      .field("kind", &self.kind)
      .finish()
  }
}

/// A runtime loads an agent brain and drives it for one iteration.
#[async_trait::async_trait]
pub trait Runtime: Send + Sync {
  /// Which implementation this is; known statically, not bound to an instance.
  fn kind() -> &'static str
  where
    Self: Sized;

  async fn run(&self, ctx: &AgentContext) -> anyhow::Result<RunOutcome>;

  /// Check the agent's current brain script for validity without running it.
  /// Used by hot reload (and startup) to validate an edit before aborting
  /// the live run.
  async fn validate(&self, ctx: &AgentContext) -> anyhow::Result<()>;
}

/// Build a runtime from opaque params. Implemented per back end; the
/// registry calls it and wraps the result into the opaque [`RuntimeEntry`].
pub trait Factory: Send + Sync + 'static {
  fn build(name: &str, params: &Value) -> anyhow::Result<Arc<Self>>
  where
    Self: Sized;
}

type FactoryFn =
  Arc<dyn Fn(&str, &Value) -> anyhow::Result<Arc<dyn Runtime>> + Send + Sync>;

/// Explicit registry of runtime back ends, keyed by static `kind`.
/// [`Registry::default`] carries the feature-gated built-ins; custom back
/// ends are added with [`Registry::register`] or
/// [`Registry::register_factory`] before the supervisor builds entries.
pub struct Registry {
  factories: std::collections::HashMap<&'static str, FactoryFn>,
}

impl Registry {
  /// An empty registry with no built-ins.
  pub fn new() -> Self {
    Self {
      factories: std::collections::HashMap::new(),
    }
  }

  fn insert(&mut self, kind: &'static str, factory: FactoryFn) {
    self.factories.insert(kind, factory);
  }

  /// Register a back-end type implementing [`Runtime`] plus [`Factory`].
  /// Rejects duplicate `kind` with an error, never overwrites.
  pub fn register<T>(&mut self) -> anyhow::Result<()>
  where
    T: Runtime + Factory,
  {
    let kind = T::kind();
    if self.factories.contains_key(kind) {
      anyhow::bail!("duplicate runtime kind {kind:?}");
    }
    let factory: FactoryFn =
      Arc::new(|name, params| Ok(T::build(name, params)? as Arc<dyn Runtime>));
    self.insert(kind, factory);
    Ok(())
  }

  /// Escape hatch for hand-built instances, test doubles holding handles,
  /// or config from elsewhere. Rejects duplicate `kind`, never overwrites.
  pub fn register_factory<F>(
    &mut self,
    kind: &'static str,
    factory: F,
  ) -> anyhow::Result<()>
  where
    F: Fn(&str, &Value) -> anyhow::Result<Arc<dyn Runtime>>
      + Send
      + Sync
      + 'static,
  {
    if self.factories.contains_key(kind) {
      anyhow::bail!("duplicate runtime kind {kind:?}");
    }
    self.insert(kind, Arc::new(factory));
    Ok(())
  }

  /// The registered kinds, sorted for deterministic errors.
  pub fn kinds(&self) -> Vec<&'static str> {
    let mut kinds: Vec<&'static str> = self.factories.keys().copied().collect();
    kinds.sort_unstable();
    kinds
  }

  /// Build a [`RuntimeEntry`] from a config entry via the registered factory.
  /// Unknown `kind` errors with the list of registered kinds.
  ///
  /// Runtime construction is config-agnostic: the caller supplies the
  /// runtime's name plus opaque params, and the registry wraps the built
  /// implementation into the entry.
  pub fn build(
    &self,
    name: &str,
    kind: &str,
    params: &Value,
  ) -> anyhow::Result<RuntimeEntry> {
    let static_kind: &'static str = self
      .factories
      .keys()
      .copied()
      .find(|k| *k == kind)
      .ok_or_else(|| {
        anyhow::anyhow!(
          "unsupported runtime kind {kind:?} (registered: {})",
          self.kinds().join(", ")
        )
      })?;
    let Some(factory) = self.factories.get(static_kind) else {
      anyhow::bail!(
        "unsupported runtime kind {kind:?} (registered: {})",
        self.kinds().join(", ")
      );
    };
    let inner = factory(name, params)?;
    Ok(RuntimeEntry {
      name: name.to_owned(),
      kind: static_kind,
      inner,
    })
  }

  /// Build one runtime entry for a single agent from the agent wiring.
  /// Errors when the agent references an unknown named runtime.
  pub fn build_for_agent(
    &self,
    cfg: &crate::config::Config,
    agent: &crate::config::AgentConfig,
  ) -> anyhow::Result<RuntimeEntry> {
    use anyhow::Context as _;
    let impl_cfg = cfg.runtime.get(&agent.runtime).with_context(|| {
      format!(
        "agent {:?} references unknown runtime {:?}",
        agent.name, agent.runtime
      )
    })?;
    self
      .build(&agent.runtime, &impl_cfg.kind, &impl_cfg.params)
      .with_context(|| format!("failed to build runtime {:?}", agent.runtime))
  }
}

impl Default for Registry {
  fn default() -> Self {
    let mut registry = Self::new();
    #[cfg(feature = "runtime-wasm")]
    {
      let _ = registry.register::<wasm::WasmRuntime>();
    }
    #[cfg(feature = "runtime-rhai")]
    {
      let _ = registry.register::<rhai::RhaiWasmRuntime>();
    }
    #[cfg(feature = "runtime-js")]
    {
      let _ = registry.register::<js::JsWasmRuntime>();
    }
    registry
  }
}

/// Register runtime back-end types into a [`Registry`].
/// Expands to one [`Registry::register`] call per type.
#[macro_export]
macro_rules! register_runtimes {
  ($registry:expr, $($t:ty),* $(,)?) => {
    $( $registry.register::<$t>()?; )*
  };
}

#[cfg(test)]
mod tests {
  use serde_json::Map;

  use super::*;

  #[test]
  fn registry_builds_known_kinds() -> anyhow::Result<()> {
    let registry = Registry::default();
    #[cfg(feature = "runtime-wasm")]
    assert!(
      registry
        .build("wasm", "wasm", &Value::Object(Map::new()))
        .is_ok()
    );
    #[cfg(feature = "runtime-rhai")]
    assert!(
      registry
        .build("rhai", "rhai", &Value::Object(Map::new()))
        .is_ok()
    );
    #[cfg(feature = "runtime-js")]
    assert!(
      registry
        .build("js", "js", &Value::Object(Map::new()))
        .is_ok()
    );
    Ok(())
  }

  #[test]
  fn registry_rejects_unknown_kind() {
    let registry = Registry::default();
    assert!(
      registry
        .build("nope", "nope", &Value::Object(Map::new()))
        .is_err()
    );
  }
}
