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
  /// Mock-only: lets the host register call-boundary injections for the
  /// scripted requests instead of a racing client task. `None` for the real
  /// endpoint, so the deterministic plumbing stays off the public
  /// [`Endpoint`] trait.
  injections: Option<Arc<dyn EndpointInjections>>,
}

/// Mock-only hook the host uses to register the scripted endpoint client's
/// requests as call-boundary injections.
pub(crate) trait EndpointInjections: Send + Sync {
  /// Register every scripted request as a pending injection.
  fn inject_requests(
    &self,
    bus: &Arc<MessageBus>,
    registry: &Arc<EndpointRegistry>,
    shutdown: &crate::shutdown::Shutdown,
  );
}

impl EndpointEntry {
  /// Build an entry from an implementation; the kind comes from the
  /// implementation itself.
  pub fn new<T: Endpoint + 'static>(endpoint: Arc<T>) -> Self {
    Self {
      kind: T::kind(),
      inner: endpoint,
      injections: None,
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

  /// The mock-only injection hook, when this entry is a mock.
  pub(crate) fn injections(&self) -> Option<&Arc<dyn EndpointInjections>> {
    self.injections.as_ref()
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

  /// A machine-readable snapshot of this instance's scripted queue state,
  /// used by the testing harness to explain a failure. Real back ends return
  /// `None`; the built-in mock reports how many scripted requests fired.
  fn snapshot(&self) -> Option<Value> {
    None
  }
}

/// Build an endpoint from opaque params. Implemented per back end; the
/// registry calls it and wraps the result into the opaque [`EndpointEntry`].
pub trait Factory: Send + Sync + 'static {
  fn build(params: &Value) -> anyhow::Result<Arc<Self>>
  where
    Self: Sized;

  /// Params of this back end that are free-form maps with user-chosen keys,
  /// relative to the impl params root. The `OMW__` environment overlay keeps
  /// the case of segments under these fields; everything else is lowercased
  /// so it still matches the config model. Defaults to none.
  fn opaque_fields() -> &'static [&'static str] {
    &[]
  }
}

type FactoryFn =
  Arc<dyn Fn(&Value) -> anyhow::Result<EndpointEntry> + Send + Sync>;

/// Explicit registry of endpoint back ends, keyed by static `kind`.
/// [`Registry::default`] carries the feature-gated built-ins; custom back
/// ends are added with [`Registry::register`] or
/// [`Registry::register_factory`] before the supervisor builds entries.
pub struct Registry {
  factories: std::collections::HashMap<&'static str, FactoryFn>,
  opaque: std::collections::HashMap<&'static str, &'static [&'static str]>,
}

impl Registry {
  /// An empty registry with no built-ins.
  pub fn new() -> Self {
    Self {
      factories: std::collections::HashMap::new(),
      opaque: std::collections::HashMap::new(),
    }
  }

  fn insert(
    &mut self,
    kind: &'static str,
    factory: FactoryFn,
    opaque: &'static [&'static str],
  ) {
    self.factories.insert(kind, factory);
    if !opaque.is_empty() {
      self.opaque.insert(kind, opaque);
    }
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
    let factory: FactoryFn = Arc::new(|params| {
      Ok(EndpointEntry {
        kind: T::kind(),
        inner: T::build(params)? as Arc<dyn Endpoint>,
        injections: None,
      })
    });
    self.insert(kind, factory, T::opaque_fields());
    Ok(())
  }

  /// Register the built-in mock endpoint. Like [`register`](Self::register)
  /// but also carries the mock's call-boundary injection hook so the scripted
  /// requests are routed at a guest call boundary.
  #[cfg(any(test, feature = "mock"))]
  pub(crate) fn register_mock<T>(&mut self) -> anyhow::Result<()>
  where
    T: Endpoint + Factory + EndpointInjections + 'static,
  {
    let kind = T::kind();
    if self.factories.contains_key(kind) {
      anyhow::bail!("duplicate endpoint kind {kind:?}");
    }
    let factory: FactoryFn = Arc::new(|params| {
      let endpoint = T::build(params)?;
      let injections: Arc<dyn EndpointInjections> = endpoint.clone();
      Ok(EndpointEntry {
        kind: T::kind(),
        inner: endpoint as Arc<dyn Endpoint>,
        injections: Some(injections),
      })
    });
    self.insert(kind, factory, T::opaque_fields());
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
    let adapted: FactoryFn = Arc::new(move |params| {
      Ok(EndpointEntry {
        kind,
        inner: factory(params)?,
        injections: None,
      })
    });
    self.insert(kind, adapted, &[]);
    Ok(())
  }

  /// The registered kinds, sorted for deterministic errors.
  pub fn kinds(&self) -> Vec<&'static str> {
    let mut kinds: Vec<&'static str> = self.factories.keys().copied().collect();
    kinds.sort_unstable();
    kinds
  }

  /// Every free-form (case-preserving) param path the registered kinds
  /// declare, relative to an entry. Each declared param is a single-segment
  /// path here.
  pub fn opaque_paths(&self) -> Vec<Vec<&'static str>> {
    self
      .opaque
      .values()
      .flat_map(|fields| fields.iter().map(|field| vec![*field]))
      .collect()
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
    let mut entry = factory(params)?;
    entry.kind = static_kind;
    Ok(entry)
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
      let _ = registry.register_mock::<mock::MockEndpoint>();
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

/// Schema for the optional `[endpoint]`: the built-in kinds enabled in this
/// build, the generic opaque escape hatch, or `null`.
pub(crate) struct EndpointImpls;

impl schemars::JsonSchema for EndpointImpls {
  fn schema_name() -> std::borrow::Cow<'static, str> {
    std::borrow::Cow::Borrowed("EndpointImpls")
  }

  fn json_schema(
    generator: &mut schemars::SchemaGenerator,
  ) -> schemars::Schema {
    let mut variants: Vec<schemars::Schema> = Vec::new();
    #[cfg(feature = "endpoint-openai")]
    variants.push(crate::schema::kind_variant::<openai::Config>(
      generator, "openai",
    ));
    #[cfg(feature = "mock")]
    variants.push(crate::schema::kind_variant::<mock::Config>(
      generator, "mock",
    ));
    variants.push(generator.subschema_for::<crate::config::ImplConfig>());
    schemars::json_schema!({
      "anyOf": [
        { "anyOf": variants },
        { "type": "null" }
      ]
    })
  }
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
