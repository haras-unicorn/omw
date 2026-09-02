//! MCP tooling over `rmcp`, the official Rust SDK for the Model Context
//! Protocol. Supports two client transports, selected by config:
//!
//! - `stdio` — spawn a server subprocess and speak newline-delimited JSON-RPC
//!   over its stdin/stdout (`TokioChildProcess`);
//! - `http` — connect to a streamable-HTTP MCP endpoint
//!   (`StreamableHttpClientTransport`).
//!
//! The transport owns the wire protocol and lifecycle (`initialize`); this
//! module only maps rmcp's typed results onto our [`Tool`] and text-joined
//! results.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use futures_util::stream::BoxStream;
use rmcp::model::{
  CallToolRequestParams, ServerNotification, SubscriptionFilter,
};
use rmcp::service::RoleClient;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{ClientLifecycleMode, ClientServiceExt};
use serde::Deserialize;
use serde_json::Value;

use super::{ResourceInfo, ResourceNotification, Tool, Tooling, ToolingEntry};

/// Which client transport to use for a single MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
  #[default]
  Stdio,
  #[serde(rename = "http")]
  Http,
}

/// Impl-specific configuration for a single MCP server.
#[derive(Clone, Deserialize)]
pub struct Config {
  /// Transport selection; defaults to `stdio`.
  #[serde(default)]
  pub transport: Option<Transport>,

  // stdio transport options.
  #[serde(default)]
  pub command: Option<String>,
  #[serde(default)]
  pub args: Vec<String>,
  #[serde(default)]
  pub env: HashMap<String, String>,

  // http transport options.
  #[serde(default)]
  pub url: Option<String>,
  /// Optional bearer token sent as the `Authorization` header.
  #[serde(default)]
  pub auth_token: Option<String>,
}

impl std::fmt::Debug for Config {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Config")
      .field("transport", &self.transport)
      .field("command", &self.command)
      .field("args", &self.args)
      .field("env_keys", &self.env.keys().collect::<Vec<_>>())
      .field("url", &self.url)
      .field(
        "auth_token",
        &self.auth_token.as_ref().map(|_| "<redacted>"),
      )
      .finish()
  }
}

/// An MCP tooling bridge over one server, owned by an rmcp [`RoleClient`].
pub struct MCPTooling {
  peer: rmcp::service::Peer<RoleClient>,
  /// Kept alive (and hence the connection open) for the life of the bridge.
  #[expect(
    dead_code,
    reason = "the running service is what keeps the transport and peer alive"
  )]
  running: Arc<rmcp::service::RunningService<RoleClient, ()>>,
}

impl std::fmt::Debug for MCPTooling {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("MCPTooling").finish_non_exhaustive()
  }
}

impl MCPTooling {
  /// Wrap an established rmcp client (already `initialize`d over some
  /// transport) as a [`MCPTooling`]. Used by [`build`] and by integration
  /// tests that connect over an in-memory transport.
  pub fn new(running: rmcp::service::RunningService<RoleClient, ()>) -> Self {
    let peer = running.peer().clone();
    Self {
      peer,
      running: Arc::new(running),
    }
  }
}

/// Build an `mcp` tooling from its opaque config params.
pub async fn build(name: &str, params: &Value) -> anyhow::Result<ToolingEntry> {
  let config = Config::deserialize(params)
    .with_context(|| format!("invalid mcp tooling config for {name:?}"))?;

  tracing::debug!(
    name,
    transport = ?config.transport.unwrap_or_default(),
    command = config.command.as_deref(),
    url = config.url.as_deref(),
    "built mcp tooling"
  );

  let running: rmcp::service::RunningService<RoleClient, ()> = connect(&config)
    .await
    .with_context(|| format!("failed to connect to MCP server {name:?}"))?;

  Ok(ToolingEntry {
    name: name.to_string(),
    kind: MCPTooling::kind(),
    tooling: Arc::new(MCPTooling::new(running)),
  })
}

/// Establish an rmcp client over the configured transport and run the
/// `initialize` lifecycle handshake. The unit type is our [`ClientHandler`];
/// this client role never handles server-initiated requests.
async fn connect(
  config: &Config,
) -> anyhow::Result<rmcp::service::RunningService<RoleClient, ()>> {
  match config.transport.unwrap_or_default() {
    Transport::Stdio => {
      let command = config
        .command
        .as_ref()
        .context("stdio transport requires `command`")?;
      let mut cmd = tokio::process::Command::new(command);
      cmd.args(&config.args);
      for (key, value) in &config.env {
        cmd.env(key, value);
      }
      let transport = TokioChildProcess::new(cmd)
        .context("failed to spawn MCP server process")?;
      ().serve_with_lifecycle(transport, ClientLifecycleMode::Initialize)
        .await
        .map_err(anyhow::Error::msg)
    }
    Transport::Http => {
      let url = config
        .url
        .as_ref()
        .context("http transport requires `url`")?;
      let mut cfg = StreamableHttpClientTransportConfig::with_uri(url.as_str());
      if let Some(token) = &config.auth_token {
        cfg.auth_header = Some(format!("Bearer {token}"));
      }
      let transport = StreamableHttpClientTransport::from_config(cfg);
      ().serve_with_lifecycle(transport, ClientLifecycleMode::Initialize)
        .await
        .map_err(anyhow::Error::msg)
    }
  }
}

#[async_trait::async_trait]
impl Tooling for MCPTooling {
  fn kind() -> &'static str {
    "mcp"
  }

  async fn list_tools(&self) -> anyhow::Result<Vec<Tool>> {
    tracing::debug!("mcp tools/list");
    let result = self
      .peer
      .list_tools(None)
      .await
      .context("MCP tools/list failed")?;
    let mut out = Vec::new();
    for t in result.tools {
      out.push(Tool {
        name: t.name.into_owned(),
        description: t.description.map(|d| d.into_owned()),
        input_schema: Value::Object(t.input_schema.as_ref().clone()),
      });
    }
    tracing::debug!(count = out.len(), "mcp tools/list returned");
    Ok(out)
  }

  async fn call_tool(&self, name: &str, args: Value) -> anyhow::Result<String> {
    tracing::trace!(name, arg_bytes = args.to_string().len(), "mcp tools/call");
    let arguments = args.as_object().cloned();
    let params = match arguments {
      Some(arguments) => {
        CallToolRequestParams::new(name.to_string()).with_arguments(arguments)
      }
      None => CallToolRequestParams::new(name.to_string()),
    };
    let result = self
      .peer
      .call_tool(params)
      .await
      .context("MCP tools/call failed")?;
    let text: Vec<String> = result
      .content
      .iter()
      .filter_map(|c| c.as_text().map(|t| t.text.clone()))
      .collect();
    let joined = text.join("\n");
    tracing::trace!(
      name,
      result_bytes = joined.len(),
      "mcp tools/call returned"
    );
    Ok(joined)
  }

  async fn list_resources(&self) -> anyhow::Result<Vec<ResourceInfo>> {
    tracing::debug!("mcp resources/list");
    let result = self
      .peer
      .list_all_resources()
      .await
      .context("MCP resources/list failed")?;
    let resources: Vec<ResourceInfo> = result
      .into_iter()
      .map(|r| ResourceInfo {
        uri: r.uri,
        name: r.name,
        description: r.description,
        mime_type: r.mime_type,
      })
      .collect();
    tracing::debug!(count = resources.len(), "mcp resources/list returned");
    Ok(resources)
  }

  async fn subscribe_resource_list(
    &self,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>
  {
    tracing::debug!("mcp resources/subscribe-list");
    let mut filter = SubscriptionFilter::new();
    filter.resources_list_changed = Some(true);
    let subscription = self
      .peer
      .listen(filter)
      .await
      .context("MCP subscriptions/listen failed")?;
    Ok(resource_stream(subscription))
  }

  async fn subscribe_resource(
    &self,
    uri: &str,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>
  {
    tracing::debug!(uri, "mcp resources/subscribe");
    let mut filter = SubscriptionFilter::new();
    filter.resource_subscriptions = Some(vec![uri.to_string()]);
    let subscription = self
      .peer
      .listen(filter)
      .await
      .context("MCP subscriptions/listen failed")?;
    Ok(resource_stream(subscription))
  }
}

/// Drain an rmcp [`Subscription`](rmcp::service::Subscription) into a
/// `BoxStream` of [`ResourceNotification`]s. Each notification arrives on
/// the stream until the subscription ends or the receiving side is dropped..
fn resource_stream(
  mut subscription: rmcp::service::Subscription,
) -> BoxStream<'static, Result<ResourceNotification, String>> {
  let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
  tokio::spawn(async move {
    loop {
      match subscription.next().await {
        Ok(Some(ServerNotification::ResourceUpdatedNotification(n))) => {
          if tx
            .send(Ok(ResourceNotification::Updated { uri: n.params.uri }))
            .is_err()
          {
            break;
          }
        }
        Ok(Some(ServerNotification::ResourceListChangedNotification(_))) => {
          if tx.send(Ok(ResourceNotification::ListChanged)).is_err() {
            break;
          }
        }
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => break,
      }
    }
  });
  Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
    rx.recv().await.map(|item| (item, rx))
  }))
}
