//! End-to-end `run_agents` integration test: a real `omw.toml` config driving
//! the whole agent runtime once, with a wiremock-backed OpenAI provider and a
//! real rmcp streamable-HTTP MCP server.
#![cfg(all(feature = "rhai", feature = "openai", feature = "mcp"))]

use std::sync::Arc;

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use omw::config::RunArgs;
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
      let id = p.chat_stream("gpt-test", [ #{ role: "user", content: "hi" } ], []);
      let out = "";
      loop {
        let e = omw::host::recv();
        if e.id == id && e.kind == "chat-delta" { out += e.payload.content; }
        if e.id == id && e.kind == "chat-end" { break; }
      }
      let t = omw::tooling::get("mcp");
      let tid = t.call_tool("echo", #{ input: "hi" });
      let tool_res = "";
      loop {
        let e = omw::host::recv();
        if e.id == tid && e.kind == "tool-result" { tool_res = e.payload.value; break; }
        if e.kind == "error" { throw e.payload; }
      }
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

  let args = RunArgs {
    config: Some(config_path),
    watch: false,
  };
  let config = args.load_config()?;
  omw::agent::run_agents(&config, false).await?;

  provider.verify().await;
  Ok(())
}

#[tokio::test]
async fn run_agents_with_watch_restarts_on_script_change() -> anyhow::Result<()>
{
  let provider = MockServer::start().await;
  Mock::given(method("POST"))
    .and(path("/v1/chat/completions"))
    .and(bearer_token("sk-test"))
    .and(wiremock::matchers::body_string_contains("ready-marker"))
    .respond_with(ResponseTemplate::new(200).set_body_string(
      "data: {\"choices\":[{\"delta\":{\"content\":\"ready\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    ))
    .mount(&provider)
    .await;
  Mock::given(method("POST"))
    .and(path("/v1/chat/completions"))
    .and(bearer_token("sk-test"))
    .and(wiremock::matchers::body_string_contains("v2-marker"))
    .respond_with(ResponseTemplate::new(200).set_body_string(
      "data: {\"choices\":[{\"delta\":{\"content\":\"v2\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    ))
    .mount(&provider)
    .await;

  let dir = tempfile::tempdir()?;
  let brain = dir.path().join("brain.rhai");
  // The brain announces itself with a `ready` chat, then parks in `recv`.
  // The ready request is the readiness signal: once it arrives, the brain
  // provably compiled, subscribed to lifecycle events, and parked — so the
  // watcher has something to interrupt. Subscribe to lifecycle events so
  // invalid-edit `error`s are observable (and the brain notices valid
  // `reload`s cooperatively).
  std::fs::write(
    &brain,
    r#"
      let lc = omw::host::subscribe_lifecycle();
      let p = omw::provider::get("openai");
      let ready = p.chat("gpt-test", [ #{ role: "user", content: "ready-marker" } ], []);
      loop {
        let e = omw::host::recv();
        if e.id == lc && e.kind == "reload" { break; }
        if e.id == lc && e.kind == "shutdown" { break; }
      }
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

        [runtime.rhai]
        kind = "rhai"

        [[agents]]
        name = "alice"
        runtime = "rhai"
        script = "{brain}"
      "#,
      base_url = provider.uri(),
      brain = brain.display(),
    ),
  )?;

  let args = RunArgs {
    config: Some(config_path),
    watch: true,
  };
  let config = args.load_config()?;
  let run =
    tokio::spawn(async move { omw::agent::run_agents(&config, true).await });

  // Startup gate runs `validate` (rhai compile via the interpreter
  // component) before the first run, so the run may lag behind spawn by
  // tens of seconds on a loaded CI VM. Poll the provider instead of
  // assuming a fixed startup latency.
  async fn wait_for_requests(
    provider: &wiremock::MockServer,
    want: usize,
    what: &str,
  ) -> anyhow::Result<Vec<wiremock::Request>> {
    let deadline = std::time::Duration::from_secs(120);
    let start = std::time::Instant::now();
    loop {
      let received = provider.received_requests().await.unwrap_or_default();
      if received.len() >= want {
        return Ok(received);
      }
      if start.elapsed() >= deadline {
        return Err(anyhow::anyhow!("timed out waiting for {what}"));
      }
      tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
  }

  // Readiness: the ready-marker chat proves the brain started and parked.
  let received = wait_for_requests(&provider, 1, "the ready chat").await?;
  let body = String::from_utf8_lossy(&received[0].body);
  assert!(body.contains("ready-marker"), "unexpected request: {body}");

  // A bad edit must never kill the good run: it is rejected by validation
  // and the live brain stays parked, so no new request arrives. The wait
  // only needs to outlast debounce (200ms) + validation; the assertion
  // holds regardless of timing since a rejected edit can only produce
  // zero new requests.
  std::fs::write(&brain, "let === ")?;
  tokio::time::sleep(std::time::Duration::from_secs(5)).await;
  let received = provider.received_requests().await.unwrap_or_default();
  assert_eq!(
    received.len(),
    1,
    "invalid edit should not restart the agent: {received:?}"
  );

  // A good edit restarts into the v2 marker, and the new run completes.
  std::fs::write(
    &brain,
    r#"
      let p = omw::provider::get("openai");
      let r = p.chat("gpt-test", [ #{ role: "user", content: "v2-marker" } ], []);
      r.content
    "#,
  )?;

  let received = wait_for_requests(&provider, 2, "the v2 chat").await?;
  let body = String::from_utf8_lossy(&received[1].body);
  assert!(body.contains("v2-marker"), "unexpected request: {body}");

  let run_outcome =
    tokio::time::timeout(std::time::Duration::from_secs(120), run)
      .await?
      .map_err(|error| anyhow::anyhow!("agent task panicked: {error}"))??;
  assert_eq!(run_outcome, ());

  let received = provider.received_requests().await.unwrap_or_default();
  assert_eq!(received.len(), 2, "expected ready + reload-triggered chats");
  Ok(())
}

#[tokio::test]
async fn run_agents_with_watch_fails_fast_on_broken_startup()
-> anyhow::Result<()> {
  let dir = tempfile::tempdir()?;
  let brain = dir.path().join("brain.rhai");
  std::fs::write(&brain, "let === ")?;

  let config_path = dir.path().join("omw.toml");
  std::fs::write(
    &config_path,
    format!(
      r#"
        [runtime.rhai]
        kind = "rhai"

        [[agents]]
        name = "alice"
        runtime = "rhai"
        script = "{brain}"
      "#,
      brain = brain.display(),
    ),
  )?;

  // Without watch a broken script fails immediately.
  let args = RunArgs {
    config: Some(config_path.clone()),
    watch: false,
  };
  let config = args.load_config()?;
  assert!(omw::agent::run_agents(&config, false).await.is_err());

  // With watch the task parks on the invalid script; a fixing edit starts
  // the agent normally.
  std::fs::write(&brain, "\"fixed\"")?;
  let config = args.load_config()?;
  tokio::time::timeout(
    std::time::Duration::from_secs(30),
    omw::agent::run_agents(&config, true),
  )
  .await??;
  Ok(())
}
