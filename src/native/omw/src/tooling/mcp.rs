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
//!
//! The connection is established lazily on first tool or resource use, so
//! construction only parses config and never touches the network. The first use
//! dials with exponential backoff (doubling from
//! `tooling_connect_backoff_start_ms` up to `tooling_connect_backoff_cap_secs`,
//! see [`Tunables`](crate::config::Tunables)); the wait is cancelled when the
//! caller goes away (reload/shutdown aborts the blocking helper, pump cancel
//! drops the event-driven call).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use futures_util::stream::BoxStream;
use rmcp::model::{
  CallToolRequestParams, ReadResourceRequestParams, ResourceContents,
  ServerNotification, SubscriptionFilter,
};
use rmcp::service::RoleClient;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{ClientLifecycleMode, ClientServiceExt};
use serde::Deserialize;
use serde_json::Value;

use super::{
  Factory, ResourceContent, ResourceInfo, ResourceNotification, Tool, Tooling,
};
use crate::config::Tunables;
use crate::secret::Secret;

/// Impl-specific configuration for a single MCP server, selected by
/// transport.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
enum Config {
  Stdio {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, Secret>,
  },
  Http {
    url: String,
    #[serde(default)]
    auth_token: Option<Secret>,
  },
}

/// A live connection: the peer to call plus the running service that keeps
/// the transport alive. Dropped when the last holder goes away.
struct Connected {
  peer: rmcp::service::Peer<RoleClient>,
  #[expect(
    dead_code,
    reason = "the running service is what keeps the transport and peer alive"
  )]
  running: Arc<rmcp::service::RunningService<RoleClient, ()>>,
}

/// An MCP tooling bridge over one server.
///
/// The bridge holds its config and dials lazily: the first tool or resource
/// use connects (with backoff) and caches the peer; later uses reuse it.
pub struct MCPTooling {
  config: Config,
  backoff_start: Duration,
  backoff_cap: Duration,
  connected: tokio::sync::Mutex<Option<Connected>>,
}

impl std::fmt::Debug for MCPTooling {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("MCPTooling").finish_non_exhaustive()
  }
}

impl MCPTooling {
  /// Wrap an established rmcp client (already `initialize`d over some
  /// transport) as a [`MCPTooling`]. Used by integration tests that connect
  /// over an in-memory transport.
  pub fn new(running: rmcp::service::RunningService<RoleClient, ()>) -> Self {
    let peer = running.peer().clone();
    Self {
      config: Config::Stdio {
        command: String::new(),
        args: Vec::new(),
        env: HashMap::new(),
      },
      backoff_start: Duration::from_millis(0),
      backoff_cap: Duration::from_millis(0),
      connected: tokio::sync::Mutex::new(Some(Connected {
        peer,
        running: Arc::new(running),
      })),
    }
  }

  /// Return the live peer, dialing first when needed and caching the result.
  /// The dial retries with backoff; a use that lands mid-backoff waits its
  /// turn on the mutex instead of dialing twice. Dropping the future (pump
  /// cancel, reload or shutdown abort) cancels the sleep or the dial.
  /// Dialing never clears: a call that already borrowed the cached peer keeps
  /// it (and keeps its server's transport alive) until it finishes, even if
  /// another use replaces the cache in the meantime; a use holding a stale
  /// peer fails plainly and the *next* use dials fresh.
  async fn peer(&self) -> anyhow::Result<rmcp::service::Peer<RoleClient>> {
    if let Some(connected) = self.connected.lock().await.as_ref() {
      return Ok(connected.peer.clone());
    }
    let mut delay = self.backoff_start;
    loop {
      match dial_once(&self.config).await {
        Ok(connected) => {
          let peer = connected.peer.clone();
          *self.connected.lock().await = Some(connected);
          return Ok(peer);
        }
        Err(error) => {
          tracing::warn!(
            error = %error,
            "failed to connect to MCP server, retrying"
          );
          if delay >= self.backoff_cap {
            return Err(error);
          }
          tokio::time::sleep(delay).await;
          delay = delay.saturating_mul(2).min(self.backoff_cap);
        }
      }
    }
  }
}

impl Factory for MCPTooling {
  fn build(
    name: &str,
    params: &Value,
    tunables: Tunables,
  ) -> anyhow::Result<Arc<Self>> {
    let config = Config::deserialize(params)
      .with_context(|| format!("invalid mcp tooling config for {name:?}"))?;
    tracing::debug!(name, config = ?config, "built mcp tooling");
    Ok(Arc::new(MCPTooling {
      config,
      backoff_start: tunables.tooling_connect_backoff_start(),
      backoff_cap: tunables.tooling_connect_backoff_cap(),
      connected: tokio::sync::Mutex::new(None),
    }))
  }
}

/// Establish an rmcp client over the configured transport and run the
/// `initialize` lifecycle handshake. The unit type is our client handler;
/// this client role never handles server-initiated requests.
async fn dial_once(config: &Config) -> anyhow::Result<Connected> {
  let running = match config {
    Config::Stdio { command, args, env } => {
      let mut cmd = tokio::process::Command::new(command);
      cmd.args(args);
      for (key, value) in env {
        cmd.env(key, value.expose());
      }
      let transport = TokioChildProcess::new(cmd)
        .context("failed to spawn MCP server process")?;
      ().serve_with_lifecycle(transport, ClientLifecycleMode::Initialize)
        .await
        .map_err(anyhow::Error::msg)
    }
    Config::Http { url, auth_token } => {
      let mut cfg = StreamableHttpClientTransportConfig::with_uri(url.as_str());
      if let Some(token) = auth_token {
        cfg.auth_header = Some(format!("Bearer {}", token.expose()));
      }
      let transport = StreamableHttpClientTransport::from_config(cfg);
      ().serve_with_lifecycle(transport, ClientLifecycleMode::Initialize)
        .await
        .map_err(anyhow::Error::msg)
    }
  }?;
  let peer = running.peer().clone();
  Ok(Connected {
    peer,
    running: Arc::new(running),
  })
}

#[async_trait::async_trait]
impl Tooling for MCPTooling {
  fn kind() -> &'static str {
    "mcp"
  }

  async fn list_tools(&self) -> anyhow::Result<Vec<Tool>> {
    tracing::debug!("mcp tools/list");
    let peer = self.peer().await?;
    let result = peer
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
    let peer = self.peer().await?;
    let result = peer
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
    let peer = self.peer().await?;
    let result = peer
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

  async fn read_resource(&self, uri: &str) -> anyhow::Result<ResourceContent> {
    tracing::debug!(uri, "mcp resources/read");
    let peer = self.peer().await?;
    let result = peer
      .read_resource(ReadResourceRequestParams::new(uri.to_string()))
      .await
      .context("MCP resources/read failed")?;
    let mut content = Vec::new();
    let mut mime_type = None;
    for c in &result.contents {
      match c {
        ResourceContents::TextResourceContents {
          text,
          mime_type: mt,
          ..
        }
        | ResourceContents::BlobResourceContents {
          blob: text,
          mime_type: mt,
          ..
        } => {
          content.push(text.clone());
          if mime_type.is_none() {
            mime_type = mt.clone();
          }
        }
        _ => {}
      }
    }
    let content = content.join("\n");
    tracing::trace!(
      uri,
      content_bytes = content.len(),
      "mcp resources/read returned"
    );
    Ok(ResourceContent {
      uri: uri.to_string(),
      mime_type,
      content,
    })
  }

  async fn subscribe_resource_list(
    &self,
  ) -> anyhow::Result<BoxStream<'static, Result<ResourceNotification, String>>>
  {
    tracing::debug!("mcp resources/subscribe-list");
    let mut filter = SubscriptionFilter::new();
    filter.resources_list_changed = Some(true);
    let peer = self.peer().await?;
    let subscription = peer
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
    let peer = self.peer().await?;
    let subscription = peer
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

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  #[test]
  fn stdio_requires_command_but_defaults_args_and_env() -> anyhow::Result<()> {
    let config = Config::deserialize(json!({
      "transport": "stdio",
      "command": "npx",
    }))?;
    let Config::Stdio { command, args, env } = config else {
      return Err(anyhow::anyhow!("expected stdio config"));
    };
    assert_eq!(command, "npx");
    assert!(args.is_empty());
    assert!(env.is_empty());
    Ok(())
  }

  #[test]
  fn http_requires_url_but_defaults_auth_token() -> anyhow::Result<()> {
    let config = Config::deserialize(json!({
      "transport": "http",
      "url": "http://127.0.0.1:8080/mcp",
    }))?;
    let Config::Http { url, auth_token } = config else {
      return Err(anyhow::anyhow!("expected http config"));
    };
    assert_eq!(url, "http://127.0.0.1:8080/mcp");
    assert!(auth_token.is_none());
    Ok(())
  }

  #[test]
  fn missing_transport_is_rejected() {
    assert!(Config::deserialize(json!({ "command": "npx" })).is_err());
  }

  #[test]
  fn stdio_without_command_is_rejected() {
    assert!(Config::deserialize(json!({ "transport": "stdio" })).is_err());
  }

  #[test]
  fn http_without_url_is_rejected() {
    assert!(Config::deserialize(json!({ "transport": "http" })).is_err());
  }

  #[test]
  fn build_parses_without_connecting() -> anyhow::Result<()> {
    let entry = super::super::Registry::default().build(
      "m",
      "mcp",
      &json!({
        "transport": "stdio",
        "command": "definitely-not-a-real-binary",
      }),
      crate::config::Tunables::default(),
    )?;
    assert_eq!(entry.name(), "m");
    assert_eq!(entry.kind(), "mcp");
    Ok(())
  }

  #[test]
  fn build_rejects_invalid_config() {
    assert!(
      super::super::Registry::default()
        .build(
          "m",
          "mcp",
          &json!({ "transport": "stdio" }),
          crate::config::Tunables::default(),
        )
        .is_err()
    );
  }

  #[tokio::test]
  async fn first_use_fails_when_server_missing() {
    let entry = super::super::Registry::default()
      .build(
        "m",
        "mcp",
        &json!({
          "transport": "stdio",
          "command": "definitely-not-a-real-binary",
        }),
        crate::config::Tunables {
          tooling_connect_backoff_start_ms: 1,
          tooling_connect_backoff_cap_secs: 0,
          ..crate::config::Tunables::default()
        },
      )
      .expect("build must not connect");
    assert!(entry.inner().list_tools().await.is_err());
  }

  #[tokio::test]
  async fn failures_are_not_cached() {
    let entry = super::super::Registry::default()
      .build(
        "m",
        "mcp",
        &json!({
          "transport": "stdio",
          "command": "definitely-not-a-real-binary",
        }),
        crate::config::Tunables {
          tooling_connect_backoff_start_ms: 1,
          tooling_connect_backoff_cap_secs: 0,
          ..crate::config::Tunables::default()
        },
      )
      .expect("build must not connect");
    // Each use redials instead of caching the failure: both fail, neither
    // panics nor poisons the bridge.
    assert!(entry.inner().list_tools().await.is_err());
    assert!(entry.inner().list_tools().await.is_err());
  }
}
