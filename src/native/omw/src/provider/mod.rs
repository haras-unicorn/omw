//! Provider abstractions.
//!
//! A provider instance is constructed from an impl-agnostic config entry
//! (a `kind` string plus opaque params) by the [`build`] factory. The
//! configured name and static kind travel with the instance in
//! [`ProviderEntry`], which is what the host hands the guest as a `provider`
//! resource handle.

use std::sync::Arc;

use anyhow::Context as _;
use futures_util::stream::BoxStream;
use serde_json::Value;

use crate::tooling::Tool;

#[cfg(test)]
pub mod mock;
pub mod openai;

/// A single chat participant role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
  System,
  User,
  Assistant,
  Tool,
}

/// A tool invocation the model asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
  pub id: String,
  pub name: String,
  pub arguments: String,
}

/// A single message in the chat history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
  pub role: Role,
  pub content: Option<String>,
  pub tool_call: Option<ToolCall>,
}

/// A streaming delta of model output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatDelta {
  pub content: Option<String>,
  pub tool_call: Option<ToolCall>,
  pub finish_reason: Option<String>,
}

/// A configured provider instance: the impl plus its config-derived name and
/// static kind. This is what the host stores in its registry and hands the
/// guest as a `provider` resource.
#[derive(Clone)]
pub struct ProviderEntry {
  pub name: String,
  pub kind: &'static str,
  pub provider: Arc<dyn Provider>,
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
/// `chat` always streams; dropping the returned stream aborts the in-flight
/// request.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
  /// Which implementation this is; known statically, not bound to an instance.
  fn kind() -> &'static str
  where
    Self: Sized;

  /// The model names this provider exposes to agents at runtime.
  async fn models(&self) -> Vec<String>;

  /// Run a chat and stream the deltas. Implementations must return an error
  /// (rather than an empty stream) on transport/auth failures before the
  /// first delta.
  async fn chat(
    &self,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: Vec<Tool>,
  ) -> anyhow::Result<BoxStream<'static, Result<ChatDelta, String>>>;
}

/// Build a provider instance from an impl-agnostic config entry.
pub fn build(
  name: &str,
  kind: &str,
  params: &Value,
) -> anyhow::Result<ProviderEntry> {
  match kind {
    "openai" => openai::build(name, params),
    #[cfg(test)]
    "mock" => mock::build(name, params),
    other => anyhow::bail!("unsupported provider kind {other:?}"),
  }
}

/// Build the registry of configured providers into entries keyed by name.
pub fn build_registry(
  cfg: &crate::config::Config,
) -> anyhow::Result<std::collections::HashMap<String, ProviderEntry>> {
  let mut providers = std::collections::HashMap::new();
  for (name, impl_cfg) in &cfg.providers {
    let entry = build(name, &impl_cfg.kind, &impl_cfg.params)
      .with_context(|| format!("failed to build provider {name:?}"))?;
    providers.insert(name.clone(), entry);
  }
  Ok(providers)
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use serde_json::json;

  use super::*;
  use crate::config::{AgentConfig, Config, ImplConfig};

  #[test]
  fn factory_builds_known_kinds() -> anyhow::Result<()> {
    let entry = build("p", "openai", &json!({}))?;
    assert_eq!(entry.name, "p");
    assert_eq!(entry.kind, "openai");

    let entry = build("m", "mock", &json!({}))?;
    assert_eq!(entry.name, "m");
    assert_eq!(entry.kind, "mock");
    Ok(())
  }

  #[test]
  fn factory_rejects_unknown_kind() {
    assert!(build("p", "nope", &json!({})).is_err());
  }

  #[test]
  fn build_registry_empty() -> anyhow::Result<()> {
    let cfg = Config {
      providers: HashMap::new(),
      tooling: HashMap::new(),
      runtime: HashMap::new(),
      agents: Vec::new(),
    };
    assert!(build_registry(&cfg)?.is_empty());
    Ok(())
  }

  #[test]
  fn build_registry_populated() -> anyhow::Result<()> {
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
      agents: vec![AgentConfig {
        name: "a".to_string(),
        runtime: "rhai".to_string(),
        script: "s".to_string(),
      }],
    };
    let reg = build_registry(&cfg)?;
    let entry = reg
      .get("m")
      .ok_or_else(|| anyhow::anyhow!("missing provider m"))?;
    assert_eq!(entry.name, "m");
    assert_eq!(entry.kind, "mock");
    Ok(())
  }
}
