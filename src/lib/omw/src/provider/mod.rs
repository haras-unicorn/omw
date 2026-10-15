//! Provider abstractions.
//!
//! A provider instance is constructed from an impl-agnostic config entry
//! (a `kind` string plus opaque params) by the [`Registry`]. The
//! configured name and static kind travel with the instance in
//! [`ProviderEntry`], which is what the host hands the guest as a `provider`
//! resource handle.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures_util::StreamExt as _;
use futures_util::stream::BoxStream;
use serde_json::Value;

use crate::tooling::Tool;

#[cfg(any(test, feature = "mock"))]
pub(crate) mod mock;
#[cfg(feature = "provider-openai")]
mod openai;

/// A tool invocation the model asked for.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ToolCall {
  pub id: String,
  pub name: String,
  pub arguments: String,
}

/// One incremental fragment of a streaming tool call, exactly as the provider
/// emits it. `id` and `name` appear on a call's first fragment; `arguments` is
/// a raw fragment of the opaque JSON string, to be concatenated across
/// fragments that share an `index`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ToolCallDelta {
  pub index: u32,
  pub id: Option<String>,
  pub name: Option<String>,
  pub arguments: Option<String>,
}

/// A single message in the chat history.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChatMessage {
  /// The sender, as an opaque role string (`"system"`, `"developer"`,
  /// `"user"`, `"assistant"`, `"tool"`, `"function"`, …).
  pub role: String,
  pub content: Option<String>,
  /// Reasoning/thinking content the model produced for this message. Sent
  /// back to the provider on assistant messages when present.
  pub reasoning: Option<String>,
  /// The tool calls this message carries (an assistant message may carry
  /// several in parallel).
  pub tool_calls: Vec<ToolCall>,
  /// For a `"tool"` message, the assistant tool call this message answers.
  pub tool_call_id: Option<String>,
}

/// A streaming delta of model output.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChatDelta {
  pub content: Option<String>,
  /// Reasoning/thinking content produced by this chunk.
  pub reasoning: Option<String>,
  /// The tool-call fragments this chunk carries, each tagged with its index.
  pub tool_calls: Vec<ToolCallDelta>,
  pub finish_reason: Option<String>,
  /// Token accounting reported by this chunk, as opaque JSON, if any.
  pub usage: Option<String>,
}

/// The in-band result of a blocking `chat` call: the concatenated text, the
/// fully-reassembled tool calls, and the terminal finish reason.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChatResult {
  pub content: Option<String>,
  /// Concatenated reasoning/thinking content, when the provider produced any.
  pub reasoning: Option<String>,
  pub tool_calls: Vec<ToolCall>,
  pub finish_reason: Option<String>,
  /// Token accounting for the response, as opaque JSON, when reported.
  pub usage: Option<String>,
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

  /// The model names this provider exposes to agents at runtime. Errors when
  /// they cannot be enumerated.
  async fn list_models(&self) -> anyhow::Result<Vec<String>>;

  async fn chat(
    &self,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: Vec<Tool>,
    params: Option<Value>,
  ) -> anyhow::Result<ChatResult> {
    let mut stream = self.chat_stream(model, messages, tools, params).await?;
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: BTreeMap<u32, ToolCall> = BTreeMap::new();
    let mut finish_reason = None;
    let mut usage = None;
    while let Some(delta) = stream.next().await {
      let delta = delta.map_err(anyhow::Error::msg)?;
      if let Some(chunk) = delta.content {
        content.push_str(&chunk);
      }
      if let Some(chunk) = delta.reasoning {
        reasoning.push_str(&chunk);
      }
      merge_tool_call_deltas(&mut tool_calls, delta.tool_calls);
      if delta.finish_reason.is_some() {
        finish_reason = delta.finish_reason;
      }
      if delta.usage.is_some() {
        usage = delta.usage;
      }
    }
    Ok(ChatResult {
      content: if content.is_empty() {
        None
      } else {
        Some(content)
      },
      reasoning: if reasoning.is_empty() {
        None
      } else {
        Some(reasoning)
      },
      tool_calls: tool_calls.into_values().collect(),
      finish_reason,
      usage,
    })
  }

  /// Run a chat and stream the deltas. Implementations must return an error
  /// (rather than an empty stream) on transport/auth failures before the
  /// first delta.
  ///
  /// `params` is an optional opaque JSON object of generation settings
  /// (temperature, max_tokens, reasoning_effort, …) forwarded to the provider
  /// and merged over its configured defaults.
  async fn chat_stream(
    &self,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: Vec<Tool>,
    params: Option<Value>,
  ) -> anyhow::Result<BoxStream<'static, Result<ChatDelta, String>>>;

  /// A machine-readable snapshot of this instance's scripted queue state,
  /// used by the testing harness to explain a failure. Real back ends return
  /// `None`; the built-in mock reports consumed vs remaining turns.
  fn snapshot(&self) -> Option<Value> {
    None
  }
}

/// Reassemble streamed tool-call fragments into complete calls, keyed by each
/// fragment's `index` so parallel calls stay distinct. An `id`/`name` lands on
/// a call's first fragment; `arguments` fragments are concatenated in arrival
/// order.
pub(crate) fn merge_tool_call_deltas(
  calls: &mut BTreeMap<u32, ToolCall>,
  fragments: Vec<ToolCallDelta>,
) {
  for fragment in fragments {
    let entry = calls.entry(fragment.index).or_insert_with(|| ToolCall {
      id: String::new(),
      name: String::new(),
      arguments: String::new(),
    });
    if let Some(id) = fragment.id {
      entry.id = id;
    }
    if let Some(name) = fragment.name {
      entry.name = name;
    }
    if let Some(arguments) = fragment.arguments {
      entry.arguments.push_str(&arguments);
    }
  }
}

/// Build a provider from opaque params. Implemented per back end; the
/// registry calls it and wraps the result into the opaque [`ProviderEntry`].
pub trait Factory: Send + Sync + 'static {
  fn build(name: &str, params: &Value) -> anyhow::Result<Arc<Self>>
  where
    Self: Sized;

  /// Params of this back end that are free-form maps with user-chosen keys,
  /// relative to the impl params root. The `OMW__` environment overlay keeps
  /// the case of segments under these fields (an MCP server's `env`, an
  /// OpenAI provider's request-body `params`, …); everything else is
  /// lowercased so it still matches the config model. Defaults to none.
  fn opaque_fields() -> &'static [&'static str] {
    &[]
  }
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
    F: Fn(&str, &Value) -> anyhow::Result<Arc<dyn Provider>>
      + Send
      + Sync
      + 'static,
  {
    if self.factories.contains_key(kind) {
      anyhow::bail!("duplicate provider kind {kind:?}");
    }
    self.insert(kind, Arc::new(factory), &[]);
    Ok(())
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

/// Schema for the `providers` map: the built-in kinds enabled in this build,
/// plus the generic opaque escape hatch for custom back ends.
pub(crate) struct ProviderImpls;

impl schemars::JsonSchema for ProviderImpls {
  fn schema_name() -> std::borrow::Cow<'static, str> {
    std::borrow::Cow::Borrowed("ProviderImpls")
  }

  fn json_schema(
    generator: &mut schemars::SchemaGenerator,
  ) -> schemars::Schema {
    let mut variants: Vec<schemars::Schema> = Vec::new();
    #[cfg(feature = "provider-openai")]
    variants.push(crate::schema::kind_variant::<openai::Config>(
      generator, "openai",
    ));
    #[cfg(feature = "mock")]
    variants.push(crate::schema::kind_variant::<mock::Config>(
      generator, "mock",
    ));
    variants.push(generator.subschema_for::<crate::config::ImplConfig>());
    schemars::json_schema!({
      "type": "object",
      "additionalProperties": { "anyOf": variants },
    })
  }
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
      agents: std::collections::BTreeMap::new(),
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
      agents: std::collections::BTreeMap::from([(
        "a".to_string(),
        AgentConfig {
          runtime: "rhai".to_string(),
          script: "s".to_string(),
        },
      )]),
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
