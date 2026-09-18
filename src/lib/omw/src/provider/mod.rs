//! Provider abstractions.
//!
//! A provider instance is constructed from an impl-agnostic config entry
//! (a `kind` string plus opaque params) by the [`Registry`]. The
//! configured name and static kind travel with the instance in
//! [`ProviderEntry`], which is what the host hands the guest as a `provider`
//! resource handle.

use std::sync::Arc;

use futures_util::StreamExt as _;
use futures_util::stream::BoxStream;
use serde_json::Value;

use crate::tooling::Tool;

#[cfg(any(test, feature = "mock"))]
pub(crate) mod mock;
#[cfg(feature = "provider-openai")]
mod openai;

/// A single chat participant role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
  System,
  User,
  Assistant,
  Tool,
}

/// A tool invocation the model asked for.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ToolCall {
  pub id: String,
  pub name: String,
  pub arguments: String,
}

/// A single message in the chat history.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChatMessage {
  pub role: Role,
  pub content: Option<String>,
  pub tool_call: Option<ToolCall>,
}

/// A streaming delta of model output.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChatDelta {
  pub content: Option<String>,
  pub tool_call: Option<ToolCall>,
  pub finish_reason: Option<String>,
}

/// The in-band result of a blocking `chat` call: the concatenated text, the
/// fully-reassembled tool calls, and the terminal finish reason.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChatResult {
  pub content: Option<String>,
  pub tool_calls: Vec<ToolCall>,
  pub finish_reason: Option<String>,
}

/// A configured provider instance: the impl plus its config-derived name and
/// static kind. This is what the host stores in its registry and hands the
/// guest as a `provider` resource.
#[derive(Clone)]
pub struct ProviderEntry {
  name: String,
  kind: &'static str,
  inner: Arc<dyn Provider>,
}

impl ProviderEntry {
  /// Build an entry from a name and an implementation; the kind comes from
  /// the implementation itself.
  pub fn new<T: Provider + 'static>(
    name: impl Into<String>,
    provider: Arc<T>,
  ) -> Self {
    Self {
      name: name.into(),
      kind: T::kind(),
      inner: provider,
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
  pub fn inner(&self) -> &Arc<dyn Provider> {
    &self.inner
  }
}

impl std::fmt::Debug for ProviderEntry {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("ProviderEntry")
      .field("name", &self.name)
      .field("kind", &self.kind)
      .finish()
  }
}

/// A provider is anything that can run a chat conversation.
///
/// `chat_stream` always streams; dropping the returned stream aborts the in-flight
/// request. `chat` blocks on `chat_stream` to completion by default, and
/// returns the accumulated [`ChatResult`] in-band.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
  /// Which implementation this is; known statically, not bound to an instance.
  fn kind() -> &'static str
  where
    Self: Sized;

  /// The model names this provider exposes to agents at runtime.
  async fn list_models(&self) -> Vec<String>;

  async fn chat(
    &self,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: Vec<Tool>,
  ) -> anyhow::Result<ChatResult> {
    let mut stream = self.chat_stream(model, messages, tools).await?;
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut finish_reason = None;
    while let Some(delta) = stream.next().await {
      let delta = delta.map_err(anyhow::Error::msg)?;
      if let Some(chunk) = delta.content {
        content.push_str(&chunk);
      }
      if let Some(tc) = delta.tool_call {
        merge_tool_call(&mut tool_calls, tc);
      }
      if delta.finish_reason.is_some() {
        finish_reason = delta.finish_reason;
      }
    }
    Ok(ChatResult {
      content: if content.is_empty() {
        None
      } else {
        Some(content)
      },
      tool_calls,
      finish_reason,
    })
  }

  /// Run a chat and stream the deltas. Implementations must return an error
  /// (rather than an empty stream) on transport/auth failures before the
  /// first delta.
  async fn chat_stream(
    &self,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: Vec<Tool>,
  ) -> anyhow::Result<BoxStream<'static, Result<ChatDelta, String>>>;
}

/// Merge one streaming ToolCall (whose arguments may be fragmented or
/// repeated across chunks) into the accumulated list, keyed by id in
/// first-seen order. A delta whose arguments extend the previous ones replaces
/// them; any other non-empty fragment is appended.
fn merge_tool_call(tool_calls: &mut Vec<ToolCall>, incoming: ToolCall) {
  if let Some(existing) = tool_calls.iter_mut().find(|c| c.id == incoming.id) {
    let args = &incoming.arguments;
    if args.starts_with(&existing.arguments) {
      existing.arguments = args.clone();
    } else if !args.is_empty() {
      existing.arguments.push_str(args);
    }
  } else {
    tool_calls.push(incoming);
  }
}

/// Build a provider from opaque params. Implemented per back end; the
/// registry calls it and wraps the result into the opaque [`ProviderEntry`].
pub trait Factory: Send + Sync + 'static {
  fn build(name: &str, params: &Value) -> anyhow::Result<Arc<Self>>
  where
    Self: Sized;
}

/// A named factory closure returning the bare implementation. The registry
/// wraps the result into the opaque [`ProviderEntry`] itself.
type FactoryFn =
  Arc<dyn Fn(&str, &Value) -> anyhow::Result<Arc<dyn Provider>> + Send + Sync>;

/// Explicit registry of provider back ends, keyed by static `kind`.
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

  /// Register a back-end type implementing [`Provider`] plus [`Factory`].
  /// Rejects duplicate `kind` with an error, never overwrites.
  pub fn register<T>(&mut self) -> anyhow::Result<()>
  where
    T: Provider + Factory,
  {
    let kind = T::kind();
    if self.factories.contains_key(kind) {
      anyhow::bail!("duplicate provider kind {kind:?}");
    }
    let factory: FactoryFn =
      Arc::new(|name, params| Ok(T::build(name, params)? as Arc<dyn Provider>));
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
    F: Fn(&str, &Value) -> anyhow::Result<Arc<dyn Provider>>
      + Send
      + Sync
      + 'static,
  {
    if self.factories.contains_key(kind) {
      anyhow::bail!("duplicate provider kind {kind:?}");
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

  /// Build a [`ProviderEntry`] from a config entry via the registered factory.
  /// Unknown `kind` errors with the list of registered kinds.
  pub fn build(
    &self,
    name: &str,
    kind: &str,
    params: &Value,
  ) -> anyhow::Result<ProviderEntry> {
    let static_kind: &'static str = self
      .factories
      .keys()
      .copied()
      .find(|k| *k == kind)
      .ok_or_else(|| {
        anyhow::anyhow!(
          "unsupported provider kind {kind:?} (registered: {})",
          self.kinds().join(", ")
        )
      })?;
    let Some(factory) = self.factories.get(static_kind) else {
      anyhow::bail!(
        "unsupported provider kind {kind:?} (registered: {})",
        self.kinds().join(", ")
      );
    };
    let inner = factory(name, params)?;
    Ok(ProviderEntry {
      name: name.to_owned(),
      kind: static_kind,
      inner,
    })
  }

  /// Build the configured providers into entries keyed by name.
  pub fn build_entries(
    &self,
    cfg: &crate::config::Config,
  ) -> anyhow::Result<std::collections::HashMap<String, ProviderEntry>> {
    use anyhow::Context as _;
    let mut providers = std::collections::HashMap::new();
    for (name, impl_cfg) in &cfg.providers {
      let entry = self
        .build(name, &impl_cfg.kind, &impl_cfg.params)
        .with_context(|| format!("failed to build provider {name:?}"))?;
      providers.insert(name.clone(), entry);
    }
    Ok(providers)
  }
}

impl Default for Registry {
  fn default() -> Self {
    let mut registry = Self::new();
    #[cfg(feature = "provider-openai")]
    {
      let _ = registry.register::<openai::OpenAIProvider>();
    }
    #[cfg(any(test, feature = "mock"))]
    {
      let _ = registry.register::<mock::MockProvider>();
    }
    registry
  }
}

/// Register provider back-end types into a [`Registry`].
/// Expands to one [`Registry::register`] call per type.
#[macro_export]
macro_rules! register_providers {
  ($registry:expr, $($t:ty),* $(,)?) => {
    $( $registry.register::<$t>()?; )*
  };
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use serde_json::json;

  use super::*;
  use crate::config::{AgentConfig, Config, ImplConfig};

  fn empty_config() -> Config {
    Config {
      providers: HashMap::new(),
      tooling: HashMap::new(),
      runtime: HashMap::new(),
      endpoint: None,
      memory: std::collections::BTreeMap::new(),
      agents: Vec::new(),
      tunables: crate::config::Tunables::default(),
    }
  }

  #[test]
  fn registry_builds_known_kinds() -> anyhow::Result<()> {
    let registry = Registry::default();
    #[cfg(feature = "provider-openai")]
    {
      let entry = registry.build("p", "openai", &json!({}))?;
      assert_eq!(entry.name(), "p");
      assert_eq!(entry.kind(), "openai");
    }

    let entry = registry.build("m", "mock", &json!({}))?;
    assert_eq!(entry.name(), "m");
    assert_eq!(entry.kind(), "mock");
    Ok(())
  }

  #[test]
  fn registry_rejects_unknown_kind() {
    let registry = Registry::default();
    assert!(registry.build("p", "nope", &json!({})).is_err());
  }

  #[test]
  fn build_entries_empty() -> anyhow::Result<()> {
    let cfg = empty_config();
    let registry = Registry::default();
    assert!(registry.build_entries(&cfg)?.is_empty());
    Ok(())
  }

  #[test]
  fn build_entries_populated() -> anyhow::Result<()> {
    let cfg = Config {
      providers: HashMap::from([(
        "m".to_string(),
        ImplConfig {
          kind: "mock".to_string(),
          params: json!({}),
        },
      )]),
      tooling: HashMap::new(),
      runtime: HashMap::new(),
      endpoint: None,
      memory: std::collections::BTreeMap::new(),
      agents: vec![AgentConfig {
        name: "a".to_string(),
        runtime: "rhai".to_string(),
        script: "s".to_string(),
      }],
      tunables: crate::config::Tunables::default(),
    };
    let registry = Registry::default();
    let entries = registry.build_entries(&cfg)?;
    let entry = entries
      .get("m")
      .ok_or_else(|| anyhow::anyhow!("missing provider m"))?;
    assert_eq!(entry.name(), "m");
    assert_eq!(entry.kind(), "mock");
    Ok(())
  }
}
