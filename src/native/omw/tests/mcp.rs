//! Integration tests for the MCP tooling backed by the real rmcp client and
//! server, connected over an in-memory `tokio::io::duplex` pair. This drives
//! the full rmcp `initialize` -> `tools/list` -> `tools/call` lifecycle (rmcp
//! owns the wire protocol) and exercises our `Tool` mapping and text joining.

use std::sync::Arc;

use omw::tooling::mcp::MCPTooling;
use omw::tooling::{Tool as OmwTool, Tooling, ToolingEntry};
use rmcp::model::{
  CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
  ListToolsResult, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt};

/// A minimal tools-only server exposing a single `echo` tool.
#[derive(Clone)]
struct EchoServer;

impl ServerHandler for EchoServer {
  fn get_info(&self) -> ServerInfo {
    ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
  }

  async fn list_tools(
    &self,
    _request: Option<rmcp::model::PaginatedRequestParams>,
    _context: RequestContext<RoleServer>,
  ) -> Result<ListToolsResult, McpError> {
    Ok(ListToolsResult::with_all_items(vec![Tool::new(
      "echo",
      "echo back the input",
      rmcp::object!({
        "type": "object",
        "properties": { "input": { "type": "string" } },
        "required": ["input"],
      }),
    )]))
  }

  async fn call_tool(
    &self,
    request: CallToolRequestParams,
    _context: RequestContext<RoleServer>,
  ) -> Result<CallToolResponse, McpError> {
    let text = request
      .arguments
      .as_ref()
      .and_then(|a| a.get("input"))
      .and_then(serde_json::Value::as_str)
      .unwrap_or_default()
      .to_string();
    Ok(CallToolResponse::from(CallToolResult::success(vec![
      ContentBlock::text(text),
    ])))
  }
}

/// Connect an rmcp client (mirroring the production MCPTooling's transport)
/// over an in-memory duplex to an `EchoServer`, and return a `Tooling`.
async fn in_memory_tooling() -> anyhow::Result<ToolingEntry> {
  let (client_io, server_io) = tokio::io::duplex(4096);
  let (client_r, client_w) = tokio::io::split(client_io);
  let (server_r, server_w) = tokio::io::split(server_io);

  // The server must drive the handshake concurrently with the client, so run
  // it in its own task rather than awaiting it (which would deadlock waiting
  // for the `initialize` the client is about to send). Park on a never-ending
  // future so the server's `RunningService` (and thus the transport) is kept
  // alive for the whole test instead of being dropped on task return.
  let _server = tokio::spawn(async move {
    #[expect(unused_variables, reason = "held alive to keep the server open")]
    let running =
      rmcp::service::serve_server(EchoServer, (server_r, server_w)).await?;
    std::future::pending::<()>().await; // hold `running` alive
    Ok::<(), anyhow::Error>(())
  });

  let running =
    ().serve((client_r, client_w))
      .await
      .map_err(anyhow::Error::msg)?;

  Ok(ToolingEntry {
    name: "in-memory".to_string(),
    kind: omw::tooling::mcp::MCPTooling::kind(),
    tooling: Arc::new(MCPTooling::new(running)),
  })
}

#[tokio::test]
async fn list_tools_parses_rmcp_tools() -> anyhow::Result<()> {
  let entry = in_memory_tooling().await?;
  let tools: Vec<OmwTool> = entry.tooling.list_tools().await?;
  assert_eq!(tools.len(), 1);
  assert_eq!(tools[0].name, "echo");
  assert_eq!(tools[0].description.as_deref(), Some("echo back the input"));
  assert!(tools[0].input_schema.is_object());
  Ok(())
}

#[tokio::test]
async fn call_tool_joins_text_content() -> anyhow::Result<()> {
  let entry = in_memory_tooling().await?;
  let result = entry
    .tooling
    .call_tool("echo", serde_json::json!({ "input": "hello" }))
    .await?;
  assert_eq!(result, "hello");
  Ok(())
}

#[tokio::test]
async fn call_tool_without_object_arguments_sends_no_arguments()
-> anyhow::Result<()> {
  let entry = in_memory_tooling().await?;
  // A non-object argument yields `None` for `arguments`; the server echoes "".
  let result = entry
    .tooling
    .call_tool("echo", serde_json::json!(42))
    .await?;
  assert_eq!(result, "");
  Ok(())
}
