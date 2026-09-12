//! Endpoint abstractions.
//!
//! An endpoint instance is constructed from an impl-agnostic config entry
//! (a `kind` string plus opaque params) by the [`build`] factory. Unlike
//! providers/tooling/runtime there is at most one endpoint per process and
//! it is optional: when absent, `subscribe-endpoint` simply errors.

use std::sync::Arc;

use serde_json::Value;

use crate::host::bus::MessageBus;
use crate::host::endpoint::EndpointRegistry;

#[cfg(feature = "endpoint-openai")]
pub mod openai;

/// A configured endpoint instance: the impl plus its static kind.
#[derive(Clone)]
pub struct EndpointEntry {
  pub kind: &'static str,
  pub endpoint: Arc<dyn Endpoint>,
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
  ) -> anyhow::Result<()>;
}

/// Build an endpoint instance from an impl-agnostic config entry.
pub fn build(kind: &str, params: &Value) -> anyhow::Result<EndpointEntry> {
  match kind {
    #[cfg(feature = "endpoint-openai")]
    "openai" => openai::build(params),
    other => anyhow::bail!("unsupported endpoint kind {other:?}"),
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  #[test]
  fn factory_builds_known_kinds() -> anyhow::Result<()> {
    #[cfg(feature = "endpoint-openai")]
    {
      let entry = build("openai", &json!({ "listen": "127.0.0.1:0" }))?;
      assert_eq!(entry.kind, "openai");
    }
    Ok(())
  }

  #[test]
  fn factory_rejects_unknown_kind() {
    assert!(build("nope", &json!({})).is_err());
  }
}
