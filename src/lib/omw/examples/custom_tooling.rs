//! Custom tooling: an in-memory `EchoTooling` that returns a canned
//! value and records what the caller sent.
//!
//! Library-only teaching material (brains use built-in kinds, not this).
//! The inline TOML wires `[tooling.echo] kind = "echo"`; the example
//! builds the `Config` programmatically, registers the back end with
//! `register_toolings!`, then runs `call_tool` and asserts on the
//! recorded name/arguments before exiting 0.

use std::sync::Arc;

use futures_util::stream::BoxStream;
use omw::prelude::*;
use serde::Deserialize;
use tokio::sync::Mutex;

/// Impl-specific config: the canned result every `call_tool` returns.
#[derive(Debug, Clone, Deserialize)]
struct EchoConfig {
  #[serde(default)]
  value: String,
}

/// One recorded `call_tool` invocation.
#[derive(Debug, Clone)]
struct EchoCall {
  name: String,
  arguments: serde_json::Value,
}

/// A scripted tooling backed by an in-memory canned value.
struct EchoTooling {
  value: String,
  calls: Arc<Mutex<Vec<EchoCall>>>,
}

impl omw::tooling::Factory for EchoTooling {
  fn build(
    _name: &str,
    params: &serde_json::Value,
    _tunables: Tunables,
  ) -> anyhow::Result<Arc<Self>> {
    let config: EchoConfig = serde_json::from_value(params.clone())?;
    Ok(Arc::new(Self {
      value: config.value,
      calls: Arc::new(Mutex::new(Vec::new())),
    }))
  }
}

impl EchoTooling {
  async fn calls(&self) -> Vec<EchoCall> {
    self.calls.lock().await.clone()
  }
}

#[async_trait::async_trait]
impl Tooling for EchoTooling {
  fn kind() -> &'static str {
    "echo"
  }

  async fn list_tools(&self) -> anyhow::Result<Vec<Tool>> {
    Ok(vec![Tool {
      name: "echo".to_string(),
      description: Some("Echoes the canned value.".to_string()),
      input_schema: serde_json::json!({ "type": "object" }),
    }])
  }

  async fn call_tool(
    &self,
    name: &str,
    args: serde_json::Value,
  ) -> anyhow::Result<String> {
    self.calls.lock().await.push(EchoCall {
      name: name.to_string(),
      arguments: args,
    });
    Ok(self.value.clone())
  }

  async fn list_resources(&self) -> anyhow::Result<Vec<ResourceInfo>> {
    Ok(Vec::new())
  }

  async fn read_resource(
    &self,
    uri: &str,
  ) -> anyhow::Result<ResourceContent> {
    anyhow::bail!("echo has no content for resource {uri:?}")
  }

  async fn subscribe_resource_list(
    &self,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>
  {
    anyhow::bail!("echo has no resource subscriptions")
  }

  async fn subscribe_resource(
    &self,
    uri: &str,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>
  {
    anyhow::bail!("echo has no subscription for resource {uri:?}")
  }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let raw = r#"
[tooling.echo]
kind = "echo"
value = "hi from echo"
"#;
  let cfg: Config = toml::from_str(raw)?;
  let mut registries = Registries::new();
  omw::register_toolings!(registries.tooling, EchoTooling);
  let entries = registries.tooling.build_entries(&cfg)?;
  let entry = entries.get("echo").ok_or_else(|| {
    anyhow::anyhow!("missing tooling \"echo\"")
  })?;
  anyhow::ensure!(
    entry.kind() == EchoTooling::kind(),
    "expected kind {:?}, got {:?}",
    EchoTooling::kind(),
    entry.kind()
  );

  let params = cfg
    .tooling
    .get("echo")
    .map(|c| c.params.clone())
    .ok_or_else(|| anyhow::anyhow!("missing tooling \"echo\""))?;
  let tooling = <EchoTooling as omw::tooling::Factory>::build(
    "echo",
    &params,
    cfg.tunables,
  )?;

  let tools = tooling.list_tools().await?;
  anyhow::ensure!(
    tools.iter().any(|t| t.name == "echo"),
    "expected an \"echo\" tool, got {tools:?}"
  );

  let args = serde_json::json!({ "input": "hi" });
  let result = tooling.call_tool("echo", args.clone()).await?;
  anyhow::ensure!(
    result == "hi from echo",
    "unexpected call_tool result: {result:?}"
  );

  let calls = tooling.calls().await;
  anyhow::ensure!(calls.len() == 1, "expected 1 call, got {}", calls.len());
  anyhow::ensure!(
    calls[0].name == "echo",
    "unexpected tool name: {:?}",
    calls[0].name
  );
  anyhow::ensure!(
    calls[0].arguments == args,
    "unexpected arguments: {:?}",
    calls[0].arguments
  );
  Ok(())
}
