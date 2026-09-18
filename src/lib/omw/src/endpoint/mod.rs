//! Endpoint abstractions.
//!
//! An endpoint instance is constructed from an impl-agnostic config entry
//! (a `kind` string plus opaque params) by the [`Registry`]. Unlike
//! providers/tooling/runtime there is at most one endpoint per process and
//! it is optional: when absent, `subscribe-endpoint` simply errors.

use std::sync::Arc;

use serde_json::Value;

use crate::host::bus::MessageBus;
use crate::host::endpoint::EndpointRegistry;

#[cfg(any(test, feature = "mock"))]
pub(crate) mod mock;
#[cfg(feature = "endpoint-openai")]
pub mod openai;

/// A configured endpoint instance: the impl plus its static kind.
#[derive(Clone)]
pub struct EndpointEntry {
  kind: &'static str,
  inner: Arc<dyn Endpoint>,
}

impl EndpointEntry {
  /// Build an entry from an implementation; the kind comes from the
  /// implementation itself.
  pub fn new<T: Endpoint + 'static>(endpoint: Arc<T>) -> Self {
    Self {
      kind: T::kind(),
      inner: endpoint,
    }
  }

  /// The static kind of this instance's implementation.
  pub fn kind(&self) -> &'static str {
    self.kind
  }

  /// The underlying implementation.
  pub fn inner(&self) -> &Arc<dyn Endpoint> {
    &self.inner
  }
}

impl std::fmt::Debug for EndpointEntry {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("EndpointEntry")
      .field("kind", &self.kind)
      .finish()
  }
}

/// An endpoint is anything that can serve agent brains over the network.
///
/// `serve` owns its serve loop; it is spawned once per process when
/// `[endpoint]` is configured.
#[async_trait::async_trait]
pub trait Endpoint: Send + Sync {
  /// Which implementation this is; known statically, not bound to an instance.
  fn kind() -> &'static str
  where
    Self: Sized;

  async fn serve(
    &self,
    bus: Arc<MessageBus>,
    registry: Arc<EndpointRegistry>,
    shutdown: crate::shutdown::Shutdown,
  ) -> anyhow::Result<()>;
}

/// Build an endpoint from opaque params. Implemented per back end; the
/// registry calls it and wraps the result into the opaque [`EndpointEntry`].
pub trait Factory: Send + Sync + 'static {
  fn build(params: &Value) -> anyhow::Result<Arc<Self>>
  where
    Self: Sized;
}

type FactoryFn =
  Arc<dyn Fn(&Value) -> anyhow::Result<Arc<dyn Endpoint>> + Send + Sync>;

/// Explicit registry of endpoint back ends, keyed by static `kind`.
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

  /// Register a back-end type implementing [`Endpoint`] plus [`Factory`].
  /// Rejects duplicate `kind` with an error, never overwrites.
  pub fn register<T>(&mut self) -> anyhow::Result<()>
  where
    T: Endpoint + Factory,
  {
    let kind = T::kind();
    if self.factories.contains_key(kind) {
      anyhow::bail!("duplicate endpoint kind {kind:?}");
    }
    let factory: FactoryFn =
      Arc::new(|params| Ok(T::build(params)? as Arc<dyn Endpoint>));
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
    F: Fn(&Value) -> anyhow::Result<Arc<dyn Endpoint>> + Send + Sync + 'static,
  {
    if self.factories.contains_key(kind) {
      anyhow::bail!("duplicate endpoint kind {kind:?}");
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

  /// Build an [`EndpointEntry`] from a config entry via the registered factory.
  /// Unknown `kind` errors with the list of registered kinds.
  pub fn build(
    &self,
    kind: &str,
    params: &Value,
  ) -> anyhow::Result<EndpointEntry> {
    let static_kind: &'static str = self
      .factories
      .keys()
      .copied()
      .find(|k| *k == kind)
      .ok_or_else(|| {
        anyhow::anyhow!(
          "unsupported endpoint kind {kind:?} (registered: {})",
          self.kinds().join(", ")
        )
      })?;
    let Some(factory) = self.factories.get(static_kind) else {
      anyhow::bail!(
        "unsupported endpoint kind {kind:?} (registered: {})",
        self.kinds().join(", ")
      );
    };
    let inner = factory(params)?;
    Ok(EndpointEntry {
      kind: static_kind,
      inner,
    })
  }

  /// Build the optional configured endpoint, if any.
  pub fn build_entry(
    &self,
    cfg: &crate::config::Config,
  ) -> anyhow::Result<Option<EndpointEntry>> {
    use anyhow::Context as _;
    cfg
      .endpoint
      .as_ref()
      .map(|endpoint| {
        self
          .build(&endpoint.kind, &endpoint.params)
          .with_context(|| {
            format!("failed to build endpoint {:?}", endpoint.kind)
          })
      })
      .transpose()
  }
}

impl Default for Registry {
  fn default() -> Self {
    let mut registry = Self::new();
    #[cfg(feature = "endpoint-openai")]
    {
      let _ = registry.register::<openai::OpenAIEndpoint>();
    }
    #[cfg(any(test, feature = "mock"))]
    {
      let _ = registry.register::<mock::MockEndpoint>();
    }
    registry
  }
}

/// Register endpoint back-end types into a [`Registry`].
/// Expands to one [`Registry::register`] call per type.
#[macro_export]
macro_rules! register_endpoints {
  ($registry:expr, $($t:ty),* $(,)?) => {
    $( $registry.register::<$t>()?; )*
  };
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  #[test]
  fn registry_builds_known_kinds() -> anyhow::Result<()> {
    #[cfg(feature = "endpoint-openai")]
    {
      let registry = Registry::default();
      let entry =
        registry.build("openai", &json!({ "listen": "127.0.0.1:0" }))?;
      assert_eq!(entry.kind(), "openai");
    }
    Ok(())
  }

  #[test]
  fn registry_rejects_unknown_kind() {
    let registry = Registry::default();
    assert!(registry.build("nope", &json!({})).is_err());
  }
}
