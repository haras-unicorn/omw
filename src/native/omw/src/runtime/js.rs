//! `JsWasmRuntime`: loads the bundled js-evaluator component, injects the
//! agent's `.js` script into the wasi environment, and evaluates it. The
//! evaluator's `omw.*` host imports route to the same global
//! provider/tooling/bus as every other runtime.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Context as _;
use serde::Deserialize;
use serde::de::IntoDeserializer;
use serde_json::Value;

use crate::host::ctx::AgentContext;
use crate::runtime::engine::{WasiConfig, WasmEngine};
use crate::runtime::{RunOutcome, Runtime};

const JS_WASM_INTERPRETER_COMPONENT_NATIVE: &[u8] =
  include_bytes!(env!("OMW_WASM_JS_INTERPRETER_COMPONENT_NATIVE"));

/// Impl-specific configuration for the js runtime.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
  #[serde(default)]
  pub interpreter: Option<PathBuf>,
  #[serde(default, flatten)]
  pub wasi: WasiConfig,
}

/// Wraps a [`WasmEngine`] pointed at the js interpreter component.
#[derive(Clone)]
pub struct JsWasmRuntime {
  #[allow(dead_code, reason = "to keep it consistent")]
  name: String,
  #[allow(dead_code, reason = "to keep it consistent")]
  config: Config,
  wasm: WasmEngine,
  /// Last successfully validated script source, kept as a TOCTOU backstop:
  /// a save landing between validate and load still runs the previous
  /// version instead of failing the run.
  last_good: Arc<Mutex<Option<String>>>,
}

impl JsWasmRuntime {
  /// Load a js evaluator component.
  pub fn new(name: String, config: Config) -> anyhow::Result<Self> {
    tracing::info!(name = %name, interpreter = ?config.interpreter, "loading js interpreter");
    let wasm = if let Some(interpreter) = config.interpreter.clone() {
      WasmEngine::from_path(&interpreter)?
    } else {
      WasmEngine::from_native_bytes(JS_WASM_INTERPRETER_COMPONENT_NATIVE)?
    };
    Ok(Self {
      name,
      config,
      wasm,
      last_good: Arc::new(Mutex::new(None)),
    })
  }
}

pub fn build(name: &str, params: &Value) -> anyhow::Result<Arc<dyn Runtime>> {
  let config = Config::deserialize(params.into_deserializer())?;
  Ok(Arc::new(JsWasmRuntime::new(name.to_owned(), config)?))
}

#[async_trait::async_trait]
impl Runtime for JsWasmRuntime {
  fn kind() -> &'static str {
    "js"
  }

  async fn run(&self, ctx: &AgentContext) -> anyhow::Result<RunOutcome> {
    let script = match tokio::fs::read_to_string(&ctx.script).await {
      Ok(script) => script,
      Err(error) => {
        if let Ok(slot) = self.last_good.lock()
          && let Some(cached) = slot.clone()
        {
          tracing::warn!(agent = %ctx.name, script = %ctx.script.display(), error = %error, "js script changed underfoot, running the last-good version");
          cached
        } else {
          tracing::error!(agent = %ctx.name, script = %ctx.script.display(), error = %error, "failed to read the js script");
          return Err(anyhow::anyhow!(
            "failed to read js script {:?}: {error}",
            ctx.script
          ));
        }
      }
    };
    tracing::debug!(agent = %ctx.name, script = %ctx.script.display(), "read the js script");

    let wasm = self.wasm.clone();
    let ctx = ctx.clone();
    let wasi = self.config.wasi.clone();

    // The wasm engine here is synchronous; push it off the tokio worker so
    // the host imports (which use `Runtime::block_on`) run on a thread that
    // is not itself inside a tokio runtime.
    let outcome =
      tokio::task::spawn_blocking(move || wasm.run(ctx, script, &wasi))
        .await
        .context("js runtime task failed")??;

    Ok(outcome.map_or(RunOutcome::Completed, RunOutcome::Exited))
  }

  async fn validate(&self, ctx: &AgentContext) -> anyhow::Result<()> {
    let script = tokio::fs::read_to_string(&ctx.script)
      .await
      .with_context(|| format!("failed to read js script {:?}", ctx.script))?;
    let wasm = self.wasm.clone();
    let ctx_clone = ctx.clone();
    let check_script = script.clone();
    let wasi = self.config.wasi.clone();
    tokio::task::spawn_blocking(move || {
      wasm.check(ctx_clone, check_script, &wasi)
    })
    .await
    .context("js check task failed")??;
    if let Ok(mut slot) = self.last_good.lock() {
      *slot = Some(script);
    }
    Ok(())
  }
}

#[cfg(test)]
mod validate_tests {
  use super::tests;
  use super::*;

  #[test]
  fn check_accepts_a_good_script_and_rejects_a_syntax_error()
  -> anyhow::Result<()> {
    use std::collections::HashMap;
    use tempfile::tempdir;
    let dir = tempdir()?;
    let good = dir.path().join("good.js");
    std::fs::write(&good, "1 + 2")?;
    let bad = dir.path().join("bad.js");
    std::fs::write(&bad, "let ==")?;
    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    tests::validate(
      &runtime,
      &tests::test_ctx(good, HashMap::new(), HashMap::new())?,
    )?;
    assert!(
      tests::validate(
        &runtime,
        &tests::test_ctx(bad, HashMap::new(), HashMap::new())?
      )
      .is_err()
    );
    Ok(())
  }

  #[test]
  fn last_good_fallback_runs_the_cached_version() -> anyhow::Result<()> {
    use std::collections::HashMap;
    use tempfile::tempdir;
    let dir = tempdir()?;
    let path = dir.path().join("brain.js");
    std::fs::write(&path, "\"cached\"")?;
    let ctx = tests::test_ctx(path.clone(), HashMap::new(), HashMap::new())?;
    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    tests::validate(&runtime, &ctx)?;
    std::fs::remove_file(&path)?;
    let outcome = tests::run(&runtime, &ctx)?;
    assert_eq!(outcome, RunOutcome::Exited("cached".to_string()));
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;
  use std::sync::Arc;

  use tempfile::tempdir;

  use super::*;
  use crate::host::bus::MessageBus;
  use crate::provider::Provider;
  use crate::provider::mock::MockProvider;
  use crate::tooling::Tooling;
  use crate::tooling::mock::MockTooling;

  /// Run the async `JsWasmRuntime::run` on a test runtime, so the bridge
  /// runtime held in `AgentContext` is dropped back on a synchronous thread
  /// (dropping a tokio runtime from an async context panics).
  pub(crate) fn run(
    runtime: &JsWasmRuntime,
    ctx: &AgentContext,
  ) -> anyhow::Result<RunOutcome> {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .enable_all()
      .build()?;
    rt.block_on(runtime.run(ctx))
  }

  pub(crate) fn validate(
    runtime: &JsWasmRuntime,
    ctx: &AgentContext,
  ) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .enable_all()
      .build()?;
    rt.block_on(runtime.validate(ctx))
  }

  pub(crate) fn test_ctx(
    script: std::path::PathBuf,
    providers: HashMap<String, crate::provider::ProviderEntry>,
    tooling: HashMap<String, crate::tooling::ToolingEntry>,
  ) -> anyhow::Result<AgentContext> {
    let bus = Arc::new(MessageBus::new());
    Ok(AgentContext::new(
      "test-agent".to_string(),
      script,
      providers,
      tooling,
      bus,
      Arc::new(crate::host::streams::StreamRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      None,
    )?)
  }

  /// Build a context wired to a caller-supplied endpoint bus + registry pair.
  fn test_endpoint_ctx(
    script: std::path::PathBuf,
    bus: Arc<MessageBus>,
    endpoint: Arc<crate::host::endpoint::EndpointRegistry>,
  ) -> anyhow::Result<AgentContext> {
    Ok(AgentContext::new(
      "test-agent".to_string(),
      script,
      HashMap::new(),
      HashMap::new(),
      bus,
      Arc::new(crate::host::streams::StreamRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Some(endpoint),
    )?)
  }

  /// Build a context with a caller-supplied bus (for multi-agent tests).
  fn test_ctx_with_bus(
    name: &str,
    script: std::path::PathBuf,
    bus: Arc<MessageBus>,
  ) -> anyhow::Result<AgentContext> {
    Ok(AgentContext::new(
      name.to_string(),
      script,
      HashMap::new(),
      HashMap::new(),
      bus,
      Arc::new(crate::host::streams::StreamRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      None,
    )?)
  }

  #[test]
  fn embedded_interpreter_evaluates_script() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("brain.js");
    std::fs::write(&path, "1 + 2")?;
    let ctx = test_ctx(path, HashMap::new(), HashMap::new())?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let outcome = run(&runtime, &ctx)?;

    // `1 + 2` evaluates to `3`, surfaced as the brain's terminal message.
    assert_eq!(outcome, RunOutcome::Exited("3".to_string()));
    Ok(())
  }

  #[test]
  fn interpreter_routes_to_provider_tooling_and_host() -> anyhow::Result<()> {
    let provider = MockProvider::noop();
    let tooling = MockTooling::noop();
    let mut providers = HashMap::new();
    providers.insert(
      "mock-provider".to_string(),
      crate::provider::ProviderEntry {
        name: "mock-provider".to_string(),
        kind: MockProvider::kind(),
        provider: provider.clone(),
      },
    );
    let mut tooling_map = HashMap::new();
    tooling_map.insert(
      "mock-tooling".to_string(),
      crate::tooling::ToolingEntry {
        name: "mock-tooling".to_string(),
        kind: MockTooling::kind(),
        tooling: tooling.clone(),
      },
    );

    let script = r#"
      let p = omw.provider.get("mock-provider");
      let id = p.chatStream("gpt-test", [ { role: "user", content: "hi" } ], []);
      let out = "";
      while (true) {
        let e = omw.host.recv();
        if (e.id === id && e.kind === "chat-delta") { out += e.payload.content; }
        if (e.id === id && e.kind === "chat-end") { break; }
      }
      let t = omw.tooling.get("mock-tooling");
      let tool_res = t.callToolBlocking("some-tool", { a: 1 });
      omw.host.log("info", "hello from test");
      out + "|" + tool_res.value
    "#;
    let dir = tempdir()?;
    let path = dir.path().join("brain.js");
    std::fs::write(&path, script)?;
    let ctx = test_ctx(path, providers, tooling_map)?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(outcome, RunOutcome::Exited("|".to_string()));

    let rt = tokio::runtime::Builder::new_multi_thread()
      .enable_all()
      .build()?;
    let calls = rt.block_on(provider.calls());
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].model, "gpt-test");
    assert_eq!(calls[0].messages.len(), 1);
    assert_eq!(calls[0].messages[0].role, crate::provider::Role::User);
    assert_eq!(calls[0].messages[0].content.as_deref(), Some("hi"));
    assert!(calls[0].tools.is_empty());

    let tool_calls = rt.block_on(tooling.calls());
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].name, "some-tool");
    assert_eq!(tool_calls[0].arguments, serde_json::json!({ "a": 1 }));
    Ok(())
  }

  #[test]
  fn tooling_call_tool_delivers_a_tool_result_event() -> anyhow::Result<()> {
    let tooling = MockTooling::noop();
    let mut tooling_map = HashMap::new();
    tooling_map.insert(
      "mock-tooling".to_string(),
      crate::tooling::ToolingEntry {
        name: "mock-tooling".to_string(),
        kind: MockTooling::kind(),
        tooling,
      },
    );

    let dir = tempdir()?;
    let path = dir.path().join("call_tool.js");
    std::fs::write(
      &path,
      r#"
        let t = omw.tooling.get("mock-tooling");
        let tid = t.callTool("some-tool", { a: 1 });
        let e = omw.host.recv();
        (e.id === tid) + "|" + e.kind + "|" + e.payload.value
      "#,
    )?;
    let ctx = test_ctx(path, HashMap::new(), tooling_map)?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(
      outcome,
      RunOutcome::Exited("true|tool-result|".to_string()),
      "callTool should return a handle and recv should yield a tool-result event"
    );
    Ok(())
  }

  #[test]
  fn provider_chat_streams_deltas_into_inbox() -> anyhow::Result<()> {
    let provider = crate::provider::build(
      "mock-provider",
      "mock",
      &serde_json::json!({ "responses": ["Hello", ", world"] }),
    )?;
    let mut providers = HashMap::new();
    providers.insert("mock-provider".to_string(), provider);

    let script = r#"
      let p = omw.provider.get("mock-provider");
      let id = p.chatStream("gpt-test", [], []);
      let out = "";
      while (true) {
        let e = omw.host.recv();
        if (e.id === id && e.kind === "chat-delta") { out += e.payload.content; }
        if (e.id === id && e.kind === "chat-end") { break; }
      }
      out
    "#;
    let dir = tempdir()?;
    let path = dir.path().join("stream.js");
    std::fs::write(&path, script)?;
    let ctx = test_ctx(path, providers, HashMap::new())?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(
      outcome,
      RunOutcome::Exited("Hello, world".to_string()),
      "chat deltas should accumulate in order until chat-end"
    );
    Ok(())
  }

  #[test]
  fn provider_chat_blocks_and_returns_a_chat_result() -> anyhow::Result<()> {
    let provider = crate::provider::build(
      "mock-provider",
      "mock",
      &serde_json::json!({ "responses": ["Hello", ", world"] }),
    )?;
    let mut providers = HashMap::new();
    providers.insert("mock-provider".to_string(), provider);

    let script = r#"
      let p = omw.provider.get("mock-provider");
      let r = p.chat("gpt-test", [], []);
      r.content + "|" + r.tool_calls.length
    "#;
    let dir = tempdir()?;
    let path = dir.path().join("chat.js");
    std::fs::write(&path, script)?;
    let ctx = test_ctx(path, providers, HashMap::new())?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(
      outcome,
      RunOutcome::Exited("Hello, world|0".to_string()),
      "blocking chat should return the accumulated content in-band"
    );
    Ok(())
  }

  #[test]
  fn host_subscribe_agent_send_recv_between_agents_on_shared_bus()
  -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());

    let dir = tempdir()?;

    // Alice subscribes to bob; the returned UUID is the handle her inbox
    // deliveries from bob will be tagged with.
    let subscribe_path = dir.path().join("subscribe.js");
    std::fs::write(&subscribe_path, r#"omw.host.subscribeAgent("bob")"#)?;
    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let alice = test_ctx_with_bus("alice", subscribe_path, Arc::clone(&bus))?;
    let outcome = run(&runtime, &alice)?;
    let sub_id = match outcome {
      RunOutcome::Exited(id) => id,
      other => anyhow::bail!("expected a subscription uuid, got {other:?}"),
    };

    // Nothing has been sent yet, so a poll returns undefined.
    let poll_path = dir.path().join("poll.js");
    std::fs::write(&poll_path, r#"omw.host.tryRecv()"#)?;
    let alice = test_ctx_with_bus("alice", poll_path, Arc::clone(&bus))?;
    let outcome = run(&runtime, &alice)?;
    assert_eq!(outcome, RunOutcome::Completed);

    // Bob sends to alice; the message lands in alice's inbox only because she
    // subscribed, tagged with the subscription UUID.
    let send_path = dir.path().join("send.js");
    std::fs::write(
      &send_path,
      r#"omw.host.sendAgent("alice", "hello from bob")"#,
    )?;
    let bob = test_ctx_with_bus("bob", send_path, Arc::clone(&bus))?;
    let outcome = run(&runtime, &bob)?;
    assert_eq!(outcome, RunOutcome::Completed);

    // Alice calls recv() to receive the envelope; its id matches the subscription UUID.
    let recv_path = dir.path().join("recv.js");
    std::fs::write(
      &recv_path,
      "let e = omw.host.recv(); e.id + \"|\" + e.kind + \"|\" + e.payload",
    )?;
    let alice = test_ctx_with_bus("alice", recv_path, bus)?;
    let outcome = run(&runtime, &alice)?;
    assert_eq!(
      outcome,
      RunOutcome::Exited(format!("{sub_id}|message|hello from bob"))
    );
    Ok(())
  }

  #[test]
  fn host_wait_for_delivers_a_timer_event() -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());

    let dir = tempdir()?;
    let path = dir.path().join("wait.js");
    std::fs::write(
      &path,
      r#"let id = omw.host.waitFor(10); let e = omw.host.recv(); (id === e.id) && e.kind === "timer""#,
    )?;

    let ctx = test_ctx_with_bus("test-agent", path, bus)?;
    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(
      outcome,
      RunOutcome::Exited("true".to_string()),
      "waitFor then recv should yield a timer event tagged with the uuid"
    );
    Ok(())
  }

  #[test]
  fn tooling_resource_subscriptions_deliver_resource_events()
  -> anyhow::Result<()> {
    let tooling =
      MockTooling::with_resource_content("file:///a", "hello-resource");

    let mut tooling_map = HashMap::new();
    tooling_map.insert(
      "mock-tooling".to_string(),
      crate::tooling::ToolingEntry {
        name: "mock-tooling".to_string(),
        kind: MockTooling::kind(),
        tooling: tooling.clone(),
      },
    );

    let dir = tempdir()?;
    let path = dir.path().join("resources.js");
    std::fs::write(
      &path,
      r#"
        let t = omw.tooling.get("mock-tooling");
        let rid = t.subscribeResource("file:///a");
        let e = omw.host.recv();
        (e.id === rid) + "|" + e.kind + "|" + e.payload.content
      "#,
    )?;
    let ctx = test_ctx(path, HashMap::new(), tooling_map)?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let outcome = run(&runtime, &ctx)?;
    let msg = match outcome {
      RunOutcome::Exited(msg) => msg,
      other => anyhow::bail!("expected an exited message, got {other:?}"),
    };
    assert!(
      msg.starts_with("true|"),
      "expected a resource-updated event, got {msg:?}"
    );
    assert_eq!(
      msg, "true|resource-updated|hello-resource",
      "expected resource-updated with payload content, got {msg:?}"
    );
    Ok(())
  }

  #[test]
  fn tooling_list_resources_surfaces_resource_info() -> anyhow::Result<()> {
    let tooling = MockTooling::noop();
    let mut tooling_map = HashMap::new();
    tooling_map.insert(
      "mock-tooling".to_string(),
      crate::tooling::ToolingEntry {
        name: "mock-tooling".to_string(),
        kind: MockTooling::kind(),
        tooling,
      },
    );

    let dir = tempdir()?;
    let path = dir.path().join("list_resources.js");
    std::fs::write(
      &path,
      r#"let t = omw.tooling.get("mock-tooling"); t.listResources()"#,
    )?;
    let ctx = test_ctx(path, HashMap::new(), tooling_map)?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let outcome = run(&runtime, &ctx)?;
    // The noop mock has no resources, so the list is an empty array.
    assert_eq!(outcome, RunOutcome::Exited("[]".to_string()));
    Ok(())
  }

  #[test]
  fn host_wait_until_in_the_past_errors() -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());

    let dir = tempdir()?;
    let path = dir.path().join("wait_past.js");
    // 1ms since epoch is far in the past relative to `now`.
    std::fs::write(&path, r#"omw.host.waitUntil(1)"#)?;

    let ctx = test_ctx_with_bus("test-agent", path, bus)?;
    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let result = run(&runtime, &ctx);
    assert!(
      result.is_err(),
      "waitUntil in the past should error, got {result:?}"
    );
    Ok(())
  }

  #[test]
  fn host_sleep_until_in_the_past_errors() -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());

    let dir = tempdir()?;
    let path = dir.path().join("sleep_past.js");
    std::fs::write(&path, r#"omw.host.sleepUntil(1)"#)?;

    let ctx = test_ctx_with_bus("test-agent", path, bus)?;
    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let result = run(&runtime, &ctx);
    assert!(
      result.is_err(),
      "sleepUntil in the past should error, got {result:?}"
    );
    Ok(())
  }

  #[test]
  fn tooling_read_resource_surfaces_resource_content() -> anyhow::Result<()> {
    let tooling =
      MockTooling::with_resource_content("file:///a", "hello-resource");

    let mut tooling_map = HashMap::new();
    tooling_map.insert(
      "mock-tooling".to_string(),
      crate::tooling::ToolingEntry {
        name: "mock-tooling".to_string(),
        kind: MockTooling::kind(),
        tooling: tooling.clone(),
      },
    );

    let dir = tempdir()?;
    let path = dir.path().join("read_resource.js");
    std::fs::write(
      &path,
      r#"let t = omw.tooling.get("mock-tooling"); let c = t.readResource("file:///a"); c.uri + "|" + c.content"#,
    )?;
    let ctx = test_ctx(path, HashMap::new(), tooling_map)?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(
      outcome,
      RunOutcome::Exited("file:///a|hello-resource".to_string()),
      "readResource should return the resource's current content"
    );
    Ok(())
  }

  #[test]
  fn host_subscribe_endpoint_routes_message_visible_to_script()
  -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(crate::host::endpoint::EndpointRegistry::new(
      Arc::clone(&bus),
    ));

    let dir = tempdir()?;
    let path = dir.path().join("endpoint.js");
    std::fs::write(
      &path,
      r#"
        let sub = omw.host.subscribeEndpoint("gpt-4o");
        let e = omw.host.recv();
        let m = e.payload.messages[0];
        let t = e.payload.tools[0];
        (e.id === sub) + "|" + e.kind + "|" + e.payload.session + "|"
          + m.role + "|" + m.content + "|" + t.name + "|"
          + t.description + "|" + t.input_schema
      "#,
    )?;
    let ctx = test_endpoint_ctx(path, Arc::clone(&bus), Arc::clone(&registry))?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let run_ctx = ctx.clone();
    let handle = std::thread::spawn(move || run(&runtime, &run_ctx));

    // Wait for the script's subscribe, then route a request into its inbox.
    // Generous budget: wasm compile + component startup on a loaded CI VM
    // can take seconds.
    for _ in 0..1000 {
      if bus.endpoint_lookup("gpt-4o").is_some() {
        break;
      }
      std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let (agent, sub_uuid) = bus
      .endpoint_lookup("gpt-4o")
      .ok_or_else(|| anyhow::anyhow!("script never subscribed"))?;
    assert_eq!(agent, "test-agent");
    let open = registry.clone().open(&agent, &sub_uuid);
    let session = open.session.clone();
    // `OpenSession` itself has no `Drop` (only `SessionRx` aborts), so the
    // session entry stays alive even though this test never drains it.
    std::mem::forget(open.rx);
    bus
      .endpoint_route(
        "gpt-4o",
        &session,
        vec![crate::provider::ChatMessage {
          role: crate::provider::Role::User,
          content: Some("hi".to_string()),
          tool_call: None,
        }],
        vec![crate::tooling::Tool {
          name: "get_weather".to_string(),
          description: Some("weather".to_string()),
          input_schema: serde_json::json!({ "type": "object" }),
        }],
      )
      .map_err(|e| anyhow::anyhow!(e))?;

    let outcome = handle
      .join()
      .map_err(|_| anyhow::anyhow!("endpoint script thread panicked"))??;
    match outcome {
      RunOutcome::Exited(msg) => {
        assert!(
          msg.starts_with("true|endpoint-message|"),
          "expected an endpoint-message event, got {msg:?}"
        );
        assert!(msg.contains(&session));
        assert!(msg.contains("user|hi|get_weather|weather"));
        assert!(msg.contains(r#"{"type":"object"}"#));
      }
      other => anyhow::bail!("expected an exited message, got {other:?}"),
    }
    Ok(())
  }

  #[test]
  fn host_stream_endpoint_sends_deltas_visible_on_session_channel()
  -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(crate::host::endpoint::EndpointRegistry::new(
      Arc::clone(&bus),
    ));
    let sub = bus
      .endpoint_subscribe("test-agent", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let open = registry.clone().open("test-agent", &sub);
    // Move the receiver out and keep it alive for the script's pushes;
    // `OpenSession` itself has no `Drop`, only `SessionRx` aborts.
    let session = open.session.clone();
    let mut rx = open.rx;

    let dir = tempdir()?;
    let path = dir.path().join("endpoint_stream.js");
    std::fs::write(
      &path,
      format!(
        r#"
        omw.host.streamEndpoint("{session}", {{ content: "Hello" }});
        omw.host.streamEndpoint("{session}", {{ finish_reason: "stop" }});
        "streamed"
      "#
      ),
    )?;
    let ctx = test_endpoint_ctx(path, Arc::clone(&bus), Arc::clone(&registry))?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(outcome, RunOutcome::Exited("streamed".to_string()));

    use crate::host::endpoint::Outbound;
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_all()
      .build()?;
    rt.block_on(async {
      match rx.recv().await {
        Some(Outbound::Delta(d)) => {
          assert_eq!(d.content.as_deref(), Some("Hello"));
        }
        other => anyhow::bail!("unexpected outbound: {other:?}"),
      }
      match rx.recv().await {
        Some(Outbound::Delta(d)) => {
          assert_eq!(d.finish_reason.as_deref(), Some("stop"));
        }
        other => anyhow::bail!("unexpected outbound: {other:?}"),
      }
      assert_eq!(rx.recv().await, Some(Outbound::Close));
      Ok::<_, anyhow::Error>(())
    })?;
    Ok(())
  }

  #[test]
  fn host_memory_get_set_remove_roundtrip_through_script() -> anyhow::Result<()>
  {
    let dir = tempdir()?;
    let path = dir.path().join("memory.js");
    std::fs::write(
      &path,
      r#"
        omw.host.memorySet("k", "v1");
        let first = omw.host.memoryGet("k");
        omw.host.memorySet("k", "v2");
        let second = omw.host.memoryGet("k");
        let deleted = omw.host.memoryRemove("k");
        let missing = omw.host.memoryGet("k");
        let deletedAgain = omw.host.memoryRemove("k");
        let missingStr = (missing === undefined) ? "none" : missing;
        first + "|" + second + "|" + deleted + "|" + missingStr + "|" + deletedAgain
      "#,
    )?;
    let ctx = test_ctx(path, HashMap::new(), HashMap::new())?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(
      outcome,
      RunOutcome::Exited("v1|v2|true|none|false".to_string()),
    );
    Ok(())
  }

  #[test]
  fn host_base64_roundtrips_through_script() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("base64.js");
    std::fs::write(
      &path,
      r#"
        let decoded = omw.host.base64Decode("AAEC/w==");
        let reencoded = omw.host.base64Encode(decoded);
        reencoded + "|" + decoded.length
      "#,
    )?;
    let ctx = test_ctx(path, HashMap::new(), HashMap::new())?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(outcome, RunOutcome::Exited("AAEC/w==|4".to_string()),);
    Ok(())
  }

  #[test]
  fn host_base64_decode_rejects_invalid_through_script() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("base64_bad.js");
    std::fs::write(&path, r#"omw.host.base64Decode("!!!")"#)?;
    let ctx = test_ctx(path, HashMap::new(), HashMap::new())?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let result = run(&runtime, &ctx);
    assert!(
      result.is_err(),
      "base64Decode on invalid input should error, got {result:?}"
    );
    Ok(())
  }

  #[test]
  fn host_memory_survives_across_runs_on_the_same_context() -> anyhow::Result<()>
  {
    let dir = tempdir()?;
    let set_path = dir.path().join("memory_set.js");
    std::fs::write(&set_path, r#"omw.host.memorySet("handle", "uuid-1")"#)?;
    let get_path = dir.path().join("memory_get.js");
    std::fs::write(&get_path, r#"omw.host.memoryGet("handle")"#)?;

    let ctx = test_ctx(set_path, HashMap::new(), HashMap::new())?;
    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(outcome, RunOutcome::Completed);

    let mut reloaded = ctx.clone();
    reloaded.script = get_path;
    let outcome = run(&runtime, &reloaded)?;
    assert_eq!(outcome, RunOutcome::Exited("uuid-1".to_string()));
    Ok(())
  }

  #[test]
  fn host_stream_endpoint_unknown_session_errors_in_script()
  -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(crate::host::endpoint::EndpointRegistry::new(
      Arc::clone(&bus),
    ));

    let dir = tempdir()?;
    let path = dir.path().join("endpoint_bad_session.js");
    std::fs::write(
      &path,
      r#"omw.host.streamEndpoint("nope", { content: "hi" })"#,
    )?;
    let ctx = test_endpoint_ctx(path, bus, registry)?;

    let runtime = JsWasmRuntime::new("".to_owned(), Config::default())?;
    let result = run(&runtime, &ctx);
    assert!(
      result.is_err(),
      "streamEndpoint on an unknown session should error, got {result:?}"
    );
    Ok(())
  }
}
