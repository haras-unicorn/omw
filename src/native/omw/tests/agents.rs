//! End-to-end `run_agents` integration test: a real `omw.toml` config driving
//! the whole agent runtime once, with a wiremock-backed OpenAI provider and a
//! real rmcp streamable-HTTP MCP server.
#![cfg(feature = "rhai")]

use std::sync::Arc;

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use omw::config::Cli;
use rmcp::model::{
  CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
  ListToolsResult, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::{
  StreamableHttpServerConfig, StreamableHttpService,
  session::local::LocalSessionManager,
};
use rmcp::{ErrorData as McpError, ServerHandler};
use wiremock::matchers::{bearer_token, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A tools-only MCP server exposing an `echo` tool, served over HTTP.
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

/// Start an rmcp streamable-HTTP server on an ephemeral port, returning its
/// `/mcp` URL and a handle that keeps the server alive for the test.
async fn start_mcp_http()
-> anyhow::Result<(String, tokio::task::JoinHandle<()>)> {
  let service = TowerToHyperService::new(StreamableHttpService::new(
    || Ok(EchoServer),
    Arc::new(LocalSessionManager::default()),
    StreamableHttpServerConfig::default(),
  ));
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
  let url = format!("http://{}/mcp", listener.local_addr()?);
  let handle = tokio::spawn(async move {
    loop {
      let Ok((stream, _)) = listener.accept().await else {
        break;
      };
      let svc = service.clone();
      tokio::spawn(async move {
        let _ = Builder::new(TokioExecutor::new())
          .serve_connection(TokioIo::new(stream), svc)
          .await;
      });
    }
  });
  Ok((url, handle))
}

#[tokio::test]
async fn run_agents_over_wiremock_openai_and_mcp_http() -> anyhow::Result<()> {
  let provider = MockServer::start().await;
  Mock::given(method("POST"))
    .and(path("/v1/chat/completions"))
    .and(bearer_token("sk-test"))
    .respond_with(ResponseTemplate::new(200).set_body_string(
      "data: {\"choices\":[{\"delta\":{\"content\":\"Hello, world\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    ))
    .expect(1)
    .mount(&provider)
    .await;

  let (mcp_url, _mcp_server) = start_mcp_http().await?;

  let dir = tempfile::tempdir()?;
  let brain = dir.path().join("brain.rhai");
  std::fs::write(
    &brain,
    r#"
      let p = omw::provider::get("openai");
      let id = p.chat("gpt-test", [ #{ role: "user", content: "hi" } ], []);
      let out = "";
      loop {
        let e = omw::host::recv();
        if e.id == id && e.kind == "delta" { out += e.payload.content; }
        if e.id == id && e.kind == "stream-end" { break; }
      }
      let t = omw::tooling::get("mcp");
      let tool_res = t.call_tool("echo", #{ input: "hi" });
      out + "|" + tool_res
    "#,
  )?;

  let config_path = dir.path().join("omw.toml");
  std::fs::write(
    &config_path,
    format!(
      r#"
        [providers.openai]
        kind = "openai"
        base_url = "{base_url}/v1"
        api_key = "sk-test"
        model = "gpt-test"

        [tooling.mcp]
        kind = "mcp"
        transport = "http"
        url = "{mcp_url}"

        [runtime.rhai]
        kind = "rhai"

        [[agents]]
        name = "alice"
        runtime = "rhai"
        script = "{brain}"
      "#,
      base_url = provider.uri(),
      mcp_url = mcp_url,
      brain = brain.display(),
    ),
  )?;

  let cli = Cli {
    command: omw::config::Command::Run,
    config: Some(config_path),
  };
  let config = cli.load_config()?;
  omw::agent::run_agents(&config).await?;

  provider.verify().await;
  Ok(())
}
