//! Tooling abstractions.
//!
//! A tooling instance is constructed from an impl-agnostic config entry (a
//! `kind` string plus opaque params) by the [`Registry`]. The configured
//! name and static kind travel with the instance in [`ToolingEntry`], which is
//! what the host hands the guest as a `tooling` resource handle.

use std::sync::Arc;

use futures_util::stream::BoxStream;
use serde_json::Value;

#[cfg(feature = "tooling-mcp")]
pub mod mcp;
#[cfg(test)]
pub(crate) mod mock;

/// A tool exposed by a tooling.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct Tool {
  pub name: String,
  pub description: Option<String>,
  /// JSON Schema for the tool's arguments.
  pub input_schema: Value,
}

/// An MCP resource (a URI-addressed, readable data value) exposed by a
/// tooling.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ResourceInfo {
  pub uri: String,
  pub name: String,
  pub description: Option<String>,
  pub mime_type: Option<String>,
}

/// The freshly read content of a single resource. `content` holds the actual
/// text for textual formats and base64-encoded bytes for anything else; the
/// guest decides which by inspecting `mime_type`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ResourceContent {
  pub uri: String,
  pub mime_type: Option<String>,
  pub content: String,
}

/// A server-initiated resource notification delivered on a subscription stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceNotification {
  /// The resource list changed (from `subscribe_resource_list`).
  ListChanged,
  /// A single resource updated in place (from `subscribe_resource`).
  Updated { uri: String },
}

/// A configured tooling instance: the impl plus its config-derived name and
/// static kind. This is what the host stores in its registry and hands the
/// guest as a `tooling` resource.
#[derive(Clone)]
pub struct ToolingEntry {
  name: String,
  kind: &'static str,
  inner: Arc<dyn Tooling>,
}

impl ToolingEntry {
  /// Build an entry from a name and an implementation; the kind comes from
  /// the implementation itself.
  pub fn new<T: Tooling + 'static>(
    name: impl Into<String>,
    tooling: Arc<T>,
  ) -> Self {
    Self {
      name: name.into(),
      kind: T::kind(),
      inner: tooling,
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
  pub fn inner(&self) -> &Arc<dyn Tooling> {
    &self.inner
  }
}

impl std::fmt::Debug for ToolingEntry {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("ToolingEntry")
      .field("name", &self.name)
      .field("kind", &self.kind)
      .finish()
  }
}

/// Tooling is anything that can enumerate and invoke tools.
#[async_trait::async_trait]
pub trait Tooling: Send + Sync {
  /// Which implementation this is; known statically, not bound to an instance.
  fn kind() -> &'static str
  where
    Self: Sized;

  /// Every tool visible on this instance.
  async fn list_tools(&self) -> anyhow::Result<Vec<Tool>>;
  /// Invoke a single tool on this instance.
  async fn call_tool(&self, name: &str, args: Value) -> anyhow::Result<String>;
  /// Every resource visible on this instance.
  async fn list_resources(&self) -> anyhow::Result<Vec<ResourceInfo>>;
  /// Read one resource's current content by URI.
  async fn read_resource(&self, uri: &str) -> anyhow::Result<ResourceContent>;
  /// Subscribe to the resource *list* changing; yielded notifications arrive
  /// on the returned stream (dropping the stream cancels the subscription).
  async fn subscribe_resource_list(
    &self,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>;

  /// Subscribe to one resource's updates; yielded notifications arrive on the
  /// returned stream (dropping the stream cancels the subscription).
  async fn subscribe_resource(
    &self,
    uri: &str,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>;
}

/// Build a tooling from opaque params plus tunables. Implemented per back
/// end; the registry calls it and wraps the result into the opaque
/// [`ToolingEntry`].
pub trait Factory: Send + Sync + 'static {
  fn build(
    name: &str,
    params: &Value,
    tunables: crate::config::Tunables,
  ) -> anyhow::Result<Arc<Self>>
  where
    Self: Sized;
}

type FactoryFn = Arc<
  dyn Fn(
      &str,
      &Value,
      crate::config::Tunables,
    ) -> anyhow::Result<Arc<dyn Tooling>>
    + Send
    + Sync,
>;

/// Explicit registry of tooling back ends, keyed by static `kind`.
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

  /// Register a back-end type implementing [`Tooling`] plus [`Factory`].
  /// Rejects duplicate `kind` with an error, never overwrites.
  pub fn register<T>(&mut self) -> anyhow::Result<()>
  where
    T: Tooling + Factory,
  {
    let kind = T::kind();
    if self.factories.contains_key(kind) {
      anyhow::bail!("duplicate tooling kind {kind:?}");
    }
    let factory: FactoryFn = Arc::new(|name, params, tunables| {
      Ok(T::build(name, params, tunables)? as Arc<dyn Tooling>)
    });
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
    F: Fn(
        &str,
        &Value,
        crate::config::Tunables,
      ) -> anyhow::Result<Arc<dyn Tooling>>
      + Send
      + Sync
      + 'static,
  {
    if self.factories.contains_key(kind) {
      anyhow::bail!("duplicate tooling kind {kind:?}");
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

  /// Build a [`ToolingEntry`] from a config entry via the registered factory.
  /// Unknown `kind` errors with the list of registered kinds.
  pub fn build(
    &self,
    name: &str,
    kind: &str,
    params: &Value,
    tunables: crate::config::Tunables,
  ) -> anyhow::Result<ToolingEntry> {
    let static_kind: &'static str = self
      .factories
      .keys()
      .copied()
      .find(|k| *k == kind)
      .ok_or_else(|| {
        anyhow::anyhow!(
          "unsupported tooling kind {kind:?} (registered: {})",
          self.kinds().join(", ")
        )
      })?;
    let Some(factory) = self.factories.get(static_kind) else {
      anyhow::bail!(
        "unsupported tooling kind {kind:?} (registered: {})",
        self.kinds().join(", ")
      );
    };
    let inner = factory(name, params, tunables)?;
    Ok(ToolingEntry {
      name: name.to_owned(),
      kind: static_kind,
      inner,
    })
  }

  /// Build the configured tooling into entries keyed by name.
  pub fn build_entries(
    &self,
    cfg: &crate::config::Config,
  ) -> anyhow::Result<std::collections::HashMap<String, ToolingEntry>> {
    use anyhow::Context as _;
    let mut tooling = std::collections::HashMap::new();
    for (name, impl_cfg) in &cfg.tooling {
      let entry = self
        .build(name, &impl_cfg.kind, &impl_cfg.params, cfg.tunables)
        .with_context(|| format!("failed to build tooling {name:?}"))?;
      tooling.insert(name.clone(), entry);
    }
    Ok(tooling)
  }
}

impl Default for Registry {
  fn default() -> Self {
    let mut registry = Self::new();
    #[cfg(feature = "tooling-mcp")]
    {
      let _ = registry.register::<mcp::MCPTooling>();
    }
    #[cfg(test)]
    {
      let _ = registry.register::<mock::MockTooling>();
    }
    registry
  }
}

/// Register tooling back-end types into a [`Registry`].
/// Expands to one [`Registry::register`] call per type.
#[macro_export]
macro_rules! register_toolings {
  ($registry:expr, $($t:ty),* $(,)?) => {
    $( $registry.register::<$t>()?; )*
  };
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use serde_json::json;

  use super::*;
  use crate::config::{Config, ImplConfig};

  #[test]
  fn registry_builds_known_kinds() -> anyhow::Result<()> {
    let tunables = crate::config::Tunables::default();
    let registry = Registry::default();
    let entry = registry.build("m", "mock", &json!({}), tunables)?;
    assert_eq!(entry.name(), "m");
    assert_eq!(entry.kind(), "mock");
    Ok(())
  }

  #[test]
  fn registry_rejects_unknown_kind() {
    let tunables = crate::config::Tunables::default();
    let registry = Registry::default();
    assert!(registry.build("t", "nope", &json!({}), tunables).is_err());
  }

  #[test]
  fn build_entries_empty() -> anyhow::Result<()> {
    let cfg = Config {
      providers: HashMap::new(),
      tooling: HashMap::new(),
      runtime: HashMap::new(),
      endpoint: None,
      agents: Vec::new(),
      tunables: crate::config::Tunables::default(),
    };
    let registry = Registry::default();
    assert!(registry.build_entries(&cfg)?.is_empty());
    Ok(())
  }

  #[test]
  fn build_entries_populated() -> anyhow::Result<()> {
    let cfg = Config {
      providers: HashMap::new(),
      tooling: HashMap::from([(
        "m".to_string(),
        ImplConfig {
          kind: "mock".to_string(),
          params: json!({}),
        },
      )]),
      runtime: HashMap::new(),
      endpoint: None,
      agents: Vec::new(),
      tunables: crate::config::Tunables::default(),
    };
    let registry = Registry::default();
    let entries = registry.build_entries(&cfg)?;
    let entry = entries
      .get("m")
      .ok_or_else(|| anyhow::anyhow!("missing tooling m"))?;
    assert_eq!(entry.name(), "m");
    assert_eq!(entry.kind(), "mock");
    Ok(())
  }
}
