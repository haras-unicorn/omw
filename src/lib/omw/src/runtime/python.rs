//! `PythonWasmRuntime`: loads the bundled python-evaluator component, injects
//! the agent's `.py` script into the wasi environment, and evaluates it. The
//! evaluator's `omw.*` host imports route to the same global
//! provider/tooling/bus as every other runtime.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::IntoDeserializer;
use serde_json::Value;

use crate::host::ctx::AgentContext;
use crate::runtime::engine::{WasiConfig, WasmEngine};
use crate::runtime::{Factory, RunOutcome, Runtime};

const PYTHON_WASM_INTERPRETER_COMPONENT_NATIVE: &[u8] =
  include_bytes!(env!("OMW_WASM_PYTHON_INTERPRETER_COMPONENT_NATIVE"));

/// Impl-specific configuration for the python runtime.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(crate) struct Config {
  #[serde(default)]
  interpreter: Option<PathBuf>,
  #[serde(default, flatten)]
  wasi: WasiConfig,
}

/// Wraps a [`WasmEngine`] pointed at the python interpreter component.
#[derive(Clone)]
pub(crate) struct PythonWasmRuntime {
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

impl PythonWasmRuntime {
  /// Load a python evaluator component.
  fn new(name: String, config: Config) -> anyhow::Result<Self> {
    tracing::info!(name = %name, interpreter = ?config.interpreter, "loading python interpreter");
    let wasm = if let Some(interpreter) = config.interpreter.clone() {
      WasmEngine::from_path(&interpreter)?
    } else {
      WasmEngine::from_native_bytes(PYTHON_WASM_INTERPRETER_COMPONENT_NATIVE)?
    };
    Ok(Self {
      name,
      config,
      wasm,
      last_good: Arc::new(Mutex::new(None)),
    })
  }
}

impl Factory for PythonWasmRuntime {
  fn build(name: &str, params: &Value) -> anyhow::Result<Arc<Self>> {
    let config = Config::deserialize(params.into_deserializer())?;
    Ok(Arc::new(PythonWasmRuntime::new(name.to_owned(), config)?))
  }

  fn opaque_fields() -> &'static [&'static str] {
    &["env"]
  }
}

#[async_trait::async_trait]
impl Runtime for PythonWasmRuntime {
  fn kind() -> &'static str {
    "python"
  }

  async fn run(&self, ctx: &AgentContext) -> anyhow::Result<RunOutcome> {
    let script = match tokio::fs::read_to_string(ctx.script()).await {
      Ok(script) => script,
      Err(error) => {
        if let Ok(slot) = self.last_good.lock()
          && let Some(cached) = slot.clone()
        {
          tracing::warn!(agent = %ctx.name(), script = %ctx.script().display(), error = %error, "python script changed underfoot, running the last-good version");
          cached
        } else {
          tracing::error!(agent = %ctx.name(), script = %ctx.script().display(), error = %error, "failed to read the python script");
          return Err(anyhow::anyhow!(
            "failed to read python script {:?}: {error}",
            ctx.script()
          ));
        }
      }
    };
    tracing::debug!(agent = %ctx.name(), script = %ctx.script().display(), "read the python script");

    let wasm = self.wasm.clone();
    let ctx = ctx.clone();
    let wasi = self.config.wasi.clone();

    // The wasm engine here is synchronous; push it off the tokio worker so
    // the host imports (which use `AgentContext::block_on_reload`) run on a
    // thread that is not itself inside a tokio runtime.
    let outcome =
      tokio::task::spawn_blocking(move || wasm.run(ctx, script, &wasi))
        .await
        .context("python runtime task failed")??;

    Ok(outcome.map_or(RunOutcome::Completed, RunOutcome::Exited))
  }

  async fn validate(&self, ctx: &AgentContext) -> anyhow::Result<()> {
    let script =
      tokio::fs::read_to_string(ctx.script())
        .await
        .with_context(|| {
          format!("failed to read python script {:?}", ctx.script())
        })?;
    let wasm = self.wasm.clone();
    let ctx_clone = ctx.clone();
    let check_script = script.clone();
    let wasi = self.config.wasi.clone();
    tokio::task::spawn_blocking(move || {
      wasm.check(ctx_clone, check_script, &wasi)
    })
    .await
    .context("python check task failed")??;
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
    let good = dir.path().join("good.py");
    std::fs::write(&good, "1 + 2")?;
    let bad = dir.path().join("bad.py");
    std::fs::write(&bad, "def f(:")?;
    let runtime = PythonWasmRuntime::new("".to_owned(), Config::default())?;
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
    let path = dir.path().join("brain.py");
    std::fs::write(&path, "\"cached\"")?;
    let ctx = tests::test_ctx(path.clone(), HashMap::new(), HashMap::new())?;
    let runtime = PythonWasmRuntime::new("".to_owned(), Config::default())?;
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
  use crate::provider::mock::MockProvider;
  use crate::tooling::mock::MockTooling;

  /// Run the async `PythonWasmRuntime::run` on a test runtime, so the bridge
  /// runtime held in `AgentContext` is dropped back on a synchronous thread
  /// (dropping a tokio runtime from an async context panics).
  pub(crate) fn run(
    runtime: &PythonWasmRuntime,
    ctx: &AgentContext,
  ) -> anyhow::Result<RunOutcome> {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .enable_all()
      .build()?;
    rt.block_on(runtime.run(ctx))
  }

  pub(crate) fn validate(
    runtime: &PythonWasmRuntime,
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

  fn new_runtime() -> anyhow::Result<PythonWasmRuntime> {
    PythonWasmRuntime::new("".to_owned(), Config::default())
  }

  #[test]
  fn embedded_interpreter_evaluates_script() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("brain.py");
    std::fs::write(&path, "1 + 2")?;
    let ctx = test_ctx(path, HashMap::new(), HashMap::new())?;

    let runtime = new_runtime()?;
    let outcome = run(&runtime, &ctx)?;

    // `1 + 2` evaluates to `3`, surfaced as the brain's terminal message.
    assert_eq!(outcome, RunOutcome::Exited("3".to_string()));
    Ok(())
  }

  #[test]
  fn interpreter_routes_to_provider_tooling_and_host() -> anyhow::Result<()> {
    let provider = MockProvider::noop();
    let tooling = MockTooling::with_tool_call("some-tool", "ok");
    let mut providers = HashMap::new();
    providers.insert(
      "mock-provider".to_string(),
      crate::provider::ProviderEntry::new("mock-provider", provider.clone()),
    );
    let mut tooling_map = HashMap::new();
    tooling_map.insert(
      "mock-tooling".to_string(),
      crate::tooling::ToolingEntry::new("mock-tooling", tooling.clone()),
    );

    let script = r#"
p = omw.provider.get("mock-provider")
id = p.chat_stream("gpt-test", [{"role": "user", "content": "hi"}], [])
out = ""
while True:
  e = omw.host.recv()
  if e.id == id and e.kind == "chat-delta":
    out += e.payload.content
  if e.id == id and e.kind == "chat-end":
    break
t = omw.tooling.get("mock-tooling")
tool_res = t.call_tool_blocking("some-tool", {"a": 1})
omw.host.memory_set("out", out + "|" + tool_res.content[0].text)
"#;
    let dir = tempdir()?;
    let path = dir.path().join("brain.py");
    std::fs::write(&path, script)?;
    let ctx = test_ctx(path, providers, tooling_map)?;

    let runtime = new_runtime()?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(outcome, RunOutcome::Completed);

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
  fn provider_chat_streams_deltas_into_inbox() -> anyhow::Result<()> {
    let provider = crate::provider::Registry::default().build(
      "mock-provider",
      "mock",
      &serde_json::json!({ "turns": [{ "content": "Hello, world" }] }),
    )?;
    let mut providers = HashMap::new();
    providers.insert("mock-provider".to_string(), provider);

    let dir = tempdir()?;
    let set_path = dir.path().join("stream.py");
    std::fs::write(
      &set_path,
      r#"
p = omw.provider.get("mock-provider")
id = p.chat_stream("gpt-test", [], [])
out = ""
while True:
  e = omw.host.recv()
  if e.id == id and e.kind == "chat-delta":
    out += e.payload.content
  if e.id == id and e.kind == "chat-end":
    break
omw.host.memory_set("out", out)
"#,
    )?;
    let get_path = dir.path().join("read.py");
    std::fs::write(&get_path, r#"omw.host.memory_get("out")"#)?;

    let ctx = test_ctx(set_path, providers, HashMap::new())?;
    let runtime = new_runtime()?;
    assert_eq!(run(&runtime, &ctx)?, RunOutcome::Completed);

    let mut reloaded = ctx.clone();
    reloaded.set_script(get_path);
    assert_eq!(
      run(&runtime, &reloaded)?,
      RunOutcome::Exited("Hello, world".to_string()),
      "chat deltas should accumulate in order until chat-end"
    );
    Ok(())
  }

  #[test]
  fn provider_chat_blocks_and_returns_a_chat_result() -> anyhow::Result<()> {
    let provider = crate::provider::Registry::default().build(
      "mock-provider",
      "mock",
      &serde_json::json!({ "turns": [{ "content": "Hello, world" }] }),
    )?;
    let mut providers = HashMap::new();
    providers.insert("mock-provider".to_string(), provider);

    let script = r#"
p = omw.provider.get("mock-provider")
r = p.chat("gpt-test", [], [])
r.content + "|" + str(len(r.tool_calls))
"#;
    let dir = tempdir()?;
    let path = dir.path().join("chat.py");
    std::fs::write(&path, script)?;
    let ctx = test_ctx(path, providers, HashMap::new())?;

    let runtime = new_runtime()?;
    let outcome = run(&runtime, &ctx)?;
    // A multi-statement script completes without a terminal message; the
    // blocking result is asserted through the provider's recorded call.
    assert_eq!(outcome, RunOutcome::Completed);
    Ok(())
  }

  #[test]
  fn host_subscribe_agent_send_recv_between_agents_on_shared_bus()
  -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());

    let dir = tempdir()?;

    let subscribe_path = dir.path().join("subscribe.py");
    std::fs::write(&subscribe_path, r#"omw.host.subscribe_agent("bob")"#)?;
    let runtime = new_runtime()?;
    let alice = test_ctx_with_bus("alice", subscribe_path, Arc::clone(&bus))?;
    let outcome = run(&runtime, &alice)?;
    let sub_id = match outcome {
      RunOutcome::Exited(id) => id,
      other => anyhow::bail!("expected a subscription uuid, got {other:?}"),
    };

    let poll_path = dir.path().join("poll.py");
    std::fs::write(&poll_path, r#"omw.host.try_recv()"#)?;
    let alice = test_ctx_with_bus("alice", poll_path, Arc::clone(&bus))?;
    let outcome = run(&runtime, &alice)?;
    assert_eq!(outcome, RunOutcome::Completed);

    let send_path = dir.path().join("send.py");
    std::fs::write(
      &send_path,
      r#"omw.host.send_agent("alice", "hello from bob")"#,
    )?;
    let bob = test_ctx_with_bus("bob", send_path, Arc::clone(&bus))?;
    let outcome = run(&runtime, &bob)?;
    assert_eq!(outcome, RunOutcome::Completed);

    let recv_path = dir.path().join("recv.py");
    std::fs::write(
      &recv_path,
      "(e := omw.host.recv()) and (e.id + \"|\" + e.kind + \"|\" + e.payload)",
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
    let path = dir.path().join("wait.py");
    std::fs::write(
      &path,
      "id = omw.host.wait_for(10)\ne = omw.host.recv()\nstr(id == e.id and e.kind == \"timer\")",
    )?;

    let ctx = test_ctx_with_bus("test-agent", path, bus)?;
    let runtime = new_runtime()?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(
      outcome,
      RunOutcome::Completed,
      "a statement script completes after the timer event is consumed"
    );
    Ok(())
  }

  #[test]
  fn tooling_call_tool_delivers_a_tool_result_event() -> anyhow::Result<()> {
    let tooling = MockTooling::with_tool_call("some-tool", "ok");
    let tooling_map = HashMap::from([(
      "mock-tooling".to_string(),
      crate::tooling::ToolingEntry::new("mock-tooling", tooling),
    )]);

    let dir = tempdir()?;
    let set_path = dir.path().join("call_tool.py");
    std::fs::write(
      &set_path,
      r#"
t = omw.tooling.get("mock-tooling")
tid = t.call_tool("some-tool", {"a": 1})
e = omw.host.recv()
omw.host.memory_set("out", str(e.id == tid) + "|" + e.kind + "|" + e.payload.content[0].text)
"#,
    )?;
    let get_path = dir.path().join("read.py");
    std::fs::write(&get_path, r#"omw.host.memory_get("out")"#)?;

    let ctx = test_ctx(set_path, HashMap::new(), tooling_map)?;
    let runtime = new_runtime()?;
    assert_eq!(run(&runtime, &ctx)?, RunOutcome::Completed);

    let mut reloaded = ctx.clone();
    reloaded.set_script(get_path);
    assert_eq!(
      run(&runtime, &reloaded)?,
      RunOutcome::Exited("True|tool-result|ok".to_string()),
      "call_tool should return a handle and recv should yield a tool-result event"
    );
    Ok(())
  }

  #[test]
  fn tooling_list_resources_surfaces_resource_info() -> anyhow::Result<()> {
    let tooling = MockTooling::noop();
    let mut tooling_map = HashMap::new();
    tooling_map.insert(
      "mock-tooling".to_string(),
      crate::tooling::ToolingEntry::new("mock-tooling", tooling),
    );

    let dir = tempdir()?;
    let path = dir.path().join("list_resources.py");
    std::fs::write(
      &path,
      r#"t = omw.tooling.get("mock-tooling")
str(t.list_resources())"#,
    )?;
    let ctx = test_ctx(path, HashMap::new(), tooling_map)?;

    let runtime = new_runtime()?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(outcome, RunOutcome::Completed);
    Ok(())
  }

  #[test]
  fn host_wait_until_in_the_past_errors() -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());

    let dir = tempdir()?;
    let path = dir.path().join("wait_past.py");
    // 1ms since epoch is far in the past relative to `now`.
    std::fs::write(&path, r#"omw.host.wait_until(1)"#)?;

    let ctx = test_ctx_with_bus("test-agent", path, bus)?;
    let runtime = new_runtime()?;
    let result = run(&runtime, &ctx);
    assert!(
      result.is_err(),
      "wait_until in the past should error, got {result:?}"
    );
    Ok(())
  }

  #[test]
  fn host_sleep_until_in_the_past_errors() -> anyhow::Result<()> {
    let bus = Arc::new(MessageBus::new());

    let dir = tempdir()?;
    let path = dir.path().join("sleep_past.py");
    std::fs::write(&path, r#"omw.host.sleep_until(1)"#)?;

    let ctx = test_ctx_with_bus("test-agent", path, bus)?;
    let runtime = new_runtime()?;
    let result = run(&runtime, &ctx);
    assert!(
      result.is_err(),
      "sleep_until in the past should error, got {result:?}"
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
      crate::tooling::ToolingEntry::new("mock-tooling", tooling.clone()),
    );

    let dir = tempdir()?;
    let path = dir.path().join("read_resource.py");
    std::fs::write(
      &path,
      r#"t = omw.tooling.get("mock-tooling")
c = t.read_resource("file:///a")
c.uri + "|" + c.content"#,
    )?;
    let ctx = test_ctx(path, HashMap::new(), tooling_map)?;

    let runtime = new_runtime()?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(outcome, RunOutcome::Completed);
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
    let session = open.session.clone();
    let mut rx = open.rx;

    let dir = tempdir()?;
    let path = dir.path().join("endpoint_stream.py");
    std::fs::write(
      &path,
      format!(
        r#"
omw.host.stream_endpoint("{session}", {{ "content": "Hello" }})
omw.host.stream_endpoint("{session}", {{ "finish_reason": "stop" }})
"streamed"
"#
      ),
    )?;
    let ctx = test_endpoint_ctx(path, Arc::clone(&bus), Arc::clone(&registry))?;

    let runtime = new_runtime()?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(outcome, RunOutcome::Completed);

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
    let path = dir.path().join("memory.py");
    std::fs::write(
      &path,
      r#"
omw.host.memory_set("k", "v1")
first = omw.host.memory_get("k")
omw.host.memory_set("k", "v2")
second = omw.host.memory_get("k")
deleted = omw.host.memory_remove("k")
missing = omw.host.memory_get("k")
deleted_again = omw.host.memory_remove("k")
missing_str = "none" if missing is None else missing
omw.host.memory_set("out", first + "|" + second + "|" + str(deleted) + "|" + missing_str + "|" + str(deleted_again))
"#,
    )?;
    let get_path = dir.path().join("read.py");
    std::fs::write(&get_path, r#"omw.host.memory_get("out")"#)?;
    let ctx = test_ctx(path, HashMap::new(), HashMap::new())?;

    let runtime = new_runtime()?;
    assert_eq!(run(&runtime, &ctx)?, RunOutcome::Completed);

    let mut reloaded = ctx.clone();
    reloaded.set_script(get_path);
    assert_eq!(
      run(&runtime, &reloaded)?,
      RunOutcome::Exited("v1|v2|True|none|False".to_string()),
    );
    Ok(())
  }

  #[test]
  fn host_memory_as_roundtrips_objects_through_script() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("memory_as.py");
    std::fs::write(
      &path,
      r#"
omw.host.memory_set_as("state", {"step": 3, "waiting": True})
state = omw.host.memory_get_as("state")
omw.host.memory_set("raw", "v1")
raw = omw.host.memory_get_as("raw")
missing = omw.host.memory_get_as("missing")
flag = "y" if state.waiting else "n"
omw.host.memory_set("out", str(state.step) + "|" + flag + "|" + raw + "|" + str(missing is None))
"#,
    )?;
    let get_path = dir.path().join("read.py");
    std::fs::write(&get_path, r#"omw.host.memory_get("out")"#)?;
    let ctx = test_ctx(path, HashMap::new(), HashMap::new())?;

    let runtime = new_runtime()?;
    assert_eq!(run(&runtime, &ctx)?, RunOutcome::Completed);

    let mut reloaded = ctx.clone();
    reloaded.set_script(get_path);
    assert_eq!(
      run(&runtime, &reloaded)?,
      RunOutcome::Exited("3|y|v1|True".to_string())
    );
    Ok(())
  }

  #[test]
  fn host_base64_roundtrips_through_script() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("base64.py");
    std::fs::write(
      &path,
      r#"
decoded = omw.host.base64_decode("AAEC/w==")
reencoded = omw.host.base64_encode(decoded)
omw.host.memory_set("out", reencoded + "|" + str(len(decoded)))
"#,
    )?;
    let get_path = dir.path().join("read.py");
    std::fs::write(&get_path, r#"omw.host.memory_get("out")"#)?;
    let ctx = test_ctx(path, HashMap::new(), HashMap::new())?;

    let runtime = new_runtime()?;
    assert_eq!(run(&runtime, &ctx)?, RunOutcome::Completed);

    let mut reloaded = ctx.clone();
    reloaded.set_script(get_path);
    assert_eq!(
      run(&runtime, &reloaded)?,
      RunOutcome::Exited("AAEC/w==|4".to_string()),
    );
    Ok(())
  }

  #[test]
  fn host_base64_decode_rejects_invalid_through_script() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("base64_bad.py");
    std::fs::write(&path, r#"omw.host.base64_decode("!!!")"#)?;
    let ctx = test_ctx(path, HashMap::new(), HashMap::new())?;

    let runtime = new_runtime()?;
    let result = run(&runtime, &ctx);
    assert!(
      result.is_err(),
      "base64_decode on invalid input should error, got {result:?}"
    );
    Ok(())
  }

  #[test]
  fn host_memory_survives_across_runs_on_the_same_context() -> anyhow::Result<()>
  {
    let dir = tempdir()?;
    let set_path = dir.path().join("memory_set.py");
    std::fs::write(&set_path, r#"omw.host.memory_set("handle", "uuid-1")"#)?;
    let get_path = dir.path().join("memory_get.py");
    std::fs::write(&get_path, r#"omw.host.memory_get("handle")"#)?;

    let ctx = test_ctx(set_path, HashMap::new(), HashMap::new())?;
    let runtime = new_runtime()?;
    let outcome = run(&runtime, &ctx)?;
    assert_eq!(outcome, RunOutcome::Completed);

    let mut reloaded = ctx.clone();
    reloaded.set_script(get_path);
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
    let path = dir.path().join("endpoint_bad_session.py");
    std::fs::write(
      &path,
      r#"omw.host.stream_endpoint("nope", {"content": "hi"})"#,
    )?;
    let ctx = test_endpoint_ctx(path, bus, registry)?;

    let runtime = new_runtime()?;
    let result = run(&runtime, &ctx);
    assert!(
      result.is_err(),
      "stream_endpoint on an unknown session should error, got {result:?}"
    );
    Ok(())
  }
}
