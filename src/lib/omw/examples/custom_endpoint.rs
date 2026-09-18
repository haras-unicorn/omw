//! Custom endpoint: a minimal `Endpoint::serve` stub that owns its
//! serve loop until shutdown.
//!
//! Library-only teaching material. The inline TOML wires `[endpoint]`
//! with `kind = "stub"` and zero agents; the example registers the
//! back end with `register_endpoints!`, boots `run_agents` with the
//! stub endpoint, and exits 0.

use std::sync::Arc;

use omw::host::bus::MessageBus;
use omw::host::endpoint::EndpointRegistry;
use omw::prelude::*;

/// A minimal endpoint that waits for shutdown, then drains.
struct StubEndpoint;

impl omw::endpoint::Factory for StubEndpoint {
  fn build(_params: &serde_json::Value) -> anyhow::Result<Arc<Self>> {
    Ok(Arc::new(Self))
  }
}

#[async_trait::async_trait]
impl Endpoint for StubEndpoint {
  fn kind() -> &'static str {
    "stub"
  }

  async fn serve(
    &self,
    bus: Arc<MessageBus>,
    registry: Arc<EndpointRegistry>,
    shutdown: Shutdown,
  ) -> anyhow::Result<()> {
    let _ = bus;
    let _ = registry;
    shutdown.wait().await;
    Ok(())
  }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let raw = r#"
[endpoint]
kind = "stub"
"#;
  let cfg: Config = toml::from_str(raw)?;
  let mut registries = Registries::new();
  omw::register_endpoints!(registries.endpoints, StubEndpoint);
  run_agents(&cfg, false, &registries).await
}
