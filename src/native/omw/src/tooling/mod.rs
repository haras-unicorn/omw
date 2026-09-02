//! Tooling abstractions.
//!
//! A tooling instance is constructed from an impl-agnostic config entry (a
//! `kind` string plus opaque params) by the [`build`] factory. The configured
//! name and static kind travel with the instance in [`ToolingEntry`], which is
//! what the host hands the guest as a `tooling` resource handle.

use std::sync::Arc;

use anyhow::Context as _;
use futures_util::stream::BoxStream;
use serde_json::Value;

pub mod mcp;
#[cfg(test)]
pub mod mock;

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
  pub name: String,
  pub kind: &'static str,
  pub tooling: Arc<dyn Tooling>,
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
  /// Subscribe to the resource *list* changing;yielded notifications arrive
  /// on the returned stream (dropping the stream cancels the subscription).
  async fn subscribe_resource_list(
    &self,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>;

  /// Subscribe to one resource's updates;yielded notifications arrive on the
  /// returned stream (dropping the stream cancels the subscription).
  async fn subscribe_resource(
    &self,
    uri: &str,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>;
}

/// Build a tooling instance from an impl-agnostic config entry.
pub async fn build(
  name: &str,
  kind: &str,
  params: &Value,
) -> anyhow::Result<ToolingEntry> {
  match kind {
    "mcp" => mcp::build(name, params).await,
    #[cfg(test)]
    "mock" => mock::build(name, params),
    other => anyhow::bail!("unsupported tooling kind {other:?}"),
  }
}

/// Build the registry of configured tooling into entries keyed by name.
pub async fn build_registry(
  cfg: &crate::config::Config,
) -> anyhow::Result<std::collections::HashMap<String, ToolingEntry>> {
  let mut tooling = std::collections::HashMap::new();
  for (name, impl_cfg) in &cfg.tooling {
    let entry = build(name, &impl_cfg.kind, &impl_cfg.params)
      .await
      .with_context(|| format!("failed to build tooling {name:?}"))?;
    tooling.insert(name.clone(), entry);
  }
  Ok(tooling)
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use serde_json::json;

  use super::*;
  use crate::config::{Config, ImplConfig};

  #[tokio::test]
  async fn factory_builds_known_kinds() -> anyhow::Result<()> {
    let entry = build("m", "mock", &json!({})).await?;
    assert_eq!(entry.name, "m");
    assert_eq!(entry.kind, "mock");
    Ok(())
  }

  #[tokio::test]
  async fn factory_rejects_unknown_kind() {
    assert!(build("t", "nope", &json!({})).await.is_err());
  }

  #[tokio::test]
  async fn build_registry_empty() -> anyhow::Result<()> {
    let cfg = Config {
      providers: HashMap::new(),
      tooling: HashMap::new(),
      runtime: HashMap::new(),
      agents: Vec::new(),
    };
    assert!(build_registry(&cfg).await?.is_empty());
    Ok(())
  }

  #[tokio::test]
  async fn build_registry_populated() -> anyhow::Result<()> {
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
      agents: Vec::new(),
    };
    let reg = build_registry(&cfg).await?;
    let entry = reg
      .get("m")
      .ok_or_else(|| anyhow::anyhow!("missing tooling m"))?;
    assert_eq!(entry.name, "m");
    assert_eq!(entry.kind, "mock");
    Ok(())
  }
}
