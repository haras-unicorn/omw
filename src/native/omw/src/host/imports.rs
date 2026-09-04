//! Host implementations of the `omw` import interfaces, bridging the
//! synchronous wasm host calls to the async provider/tooling implementations
//! and the message bus.
//!
//! Because the wasm engine runs synchronously here, async work is bridged two
//! ways:
//!   * `provider.chat` spawns a pump task (see `host/streams.rs`) on the
//!     shared tokio runtime that delivers `chat-delta` / `stream-end` events into the
//!     agent's inbox.

//!   * `tooling.*` and `host.*` results are obtained with
//!     [`Runtime::block_on`], which is only legal on threads that are not
//!     themselves inside a tokio runtime (i.e. our `spawn_blocking` wasm
//!     thread).

use std::sync::Arc;
use std::time::Duration;

use wasmtime::component::Resource;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::bindings::omw::omw::host as host_bindings;
use crate::bindings::omw::omw::provider as provider_bindings;
use crate::bindings::omw::omw::tooling as tooling_bindings;
use crate::bindings::omw::omw::types as types_bindings;
use crate::host::ctx::AgentContext;
use crate::host::events::Event;
use crate::provider::{ChatDelta, ChatMessage, ProviderEntry, Role, ToolCall};
use crate::tooling::{ResourceContent, ResourceInfo, Tool, ToolingEntry};

/// The store-side host that satisfies all three import interfaces. It also
/// implements [`WasiView`] so the guest's implicit `wasi:cli/environment`
/// import is satisfied by `wasmtime-wasi`.
pub struct Host {
  pub ctx: AgentContext,
  pub table: wasmtime::component::ResourceTable,
  pub wasi: WasiCtx,
}

impl WasiView for Host {
  fn ctx(&mut self) -> WasiCtxView<'_> {
    WasiCtxView {
      ctx: &mut self.wasi,
      table: &mut self.table,
    }
  }
}

impl provider_bindings::Host for Host {
  fn get(&mut self, name: String) -> Result<Resource<ProviderEntry>, String> {
    let entry = match self.ctx.providers.get(&name).cloned() {
      Some(entry) => entry,
      None => {
        tracing::warn!(agent = %self.ctx.name, provider = %name, "provider get miss");
        return Err(format!("no such provider {name:?}"));
      }
    };
    tracing::debug!(agent = %self.ctx.name, provider = %name, "provider get hit");
    self.table.push(entry).map_err(|e| e.to_string())
  }
}

impl provider_bindings::HostProvider for Host {
  fn name(&mut self, self_: Resource<ProviderEntry>) -> String {
    self
      .table
      .get(&self_)
      .map(|e| e.name.clone())
      .unwrap_or_default()
  }

  fn kind(&mut self, self_: Resource<ProviderEntry>) -> String {
    self
      .table
      .get(&self_)
      .map(|e| e.kind.to_string())
      .unwrap_or_default()
  }

  fn models(&mut self, self_: Resource<ProviderEntry>) -> Vec<String> {
    let Some(entry) = self.table.get(&self_).ok() else {
      return Vec::new();
    };
    let provider = Arc::clone(&entry.provider);
    let rt = Arc::clone(&self.ctx.rt());
    rt.block_on(provider.models())
  }

  fn chat(
    &mut self,
    self_: Resource<ProviderEntry>,
    model: String,
    messages: Vec<provider_bindings::ChatMessage>,
    tools: Vec<tooling_bindings::Tool>,
  ) -> Result<String, String> {
    let entry = self.table.get(&self_).map_err(|e| e.to_string())?;
    let msgs: Vec<ChatMessage> = messages.into_iter().map(in_msg).collect();
    let tools: Vec<Tool> = tools.into_iter().map(in_tool).collect();
    let provider = Arc::clone(&entry.provider);
    let rt = Arc::clone(&self.ctx.rt());
    let bus = Arc::clone(&self.ctx.bus);
    let streams = Arc::clone(&self.ctx.streams);
    let name = self.ctx.name.clone();
    let uuid = crate::host::bus::new_uuid();
    tracing::debug!(
      agent = %self.ctx.name,
      provider = %entry.name,
      uuid = %uuid,
      "opening a chat stream"
    );
    crate::host::streams::spawn_pump(
      provider,
      rt,
      bus,
      streams,
      name,
      uuid.clone(),
      model,
      msgs,
      tools,
    );
    Ok(uuid)
  }

  fn is_open(&mut self, _self_: Resource<ProviderEntry>, uuid: String) -> bool {
    self.ctx.streams.is_open(&uuid)
  }

  fn cancel(&mut self, _self_: Resource<ProviderEntry>, uuid: String) {
    self.ctx.streams.cancel(&uuid);
  }

  fn drop(&mut self, self_: Resource<ProviderEntry>) -> wasmtime::Result<()> {
    self.table.delete(self_)?;
    Ok(())
  }
}

impl tooling_bindings::Host for Host {
  fn get(&mut self, name: String) -> Result<Resource<ToolingEntry>, String> {
    let entry = match self.ctx.tooling.get(&name).cloned() {
      Some(entry) => entry,
      None => {
        tracing::warn!(agent = %self.ctx.name, tooling = %name, "tooling get miss");
        return Err(format!("no such tooling {name:?}"));
      }
    };
    tracing::debug!(agent = %self.ctx.name, tooling = %name, "tooling get hit");
    self.table.push(entry).map_err(|e| e.to_string())
  }
}

impl tooling_bindings::HostTooling for Host {
  fn name(&mut self, self_: Resource<ToolingEntry>) -> String {
    self
      .table
      .get(&self_)
      .map(|e| e.name.clone())
      .unwrap_or_default()
  }

  fn kind(&mut self, self_: Resource<ToolingEntry>) -> String {
    self
      .table
      .get(&self_)
      .map(|e| e.kind.to_string())
      .unwrap_or_default()
  }

  fn list_tools(
    &mut self,
    self_: Resource<ToolingEntry>,
  ) -> Result<Vec<tooling_bindings::Tool>, String> {
    let entry = self.table.get(&self_).map_err(|e| e.to_string())?;
    let tooling = Arc::clone(&entry.tooling);
    let agent = self.ctx.name.clone();
    tracing::debug!(agent = %agent, tooling = %entry.name, "listing tools");
    let rt = Arc::clone(&self.ctx.rt());
    let tools = rt
      .block_on(tooling.list_tools())
      .map_err(|e| e.to_string())?;
    tracing::debug!(agent = %agent, tooling = %entry.name, count = tools.len(), "listed tools");
    Ok(tools.into_iter().map(Tool::into).collect())
  }

  fn call_tool(
    &mut self,
    self_: Resource<ToolingEntry>,
    name: String,
    arguments: String,
  ) -> Result<String, String> {
    let entry = self.table.get(&self_).map_err(|e| e.to_string())?;
    let tooling = Arc::clone(&entry.tooling);
    let agent = self.ctx.name.clone();
    let args =
      serde_json::from_str(&arguments).unwrap_or(serde_json::Value::Null);
    tracing::trace!(
      agent = %agent,
      tooling = %entry.name,
      tool = %name,
      arg_bytes = arguments.len(),
      "calling a tool"
    );
    let rt = Arc::clone(&self.ctx.rt());
    let result = rt
      .block_on(tooling.call_tool(&name, args))
      .map_err(|e| e.to_string())?;
    tracing::trace!(
      agent = %agent,
      tooling = %entry.name,
      tool = %name,
      result_bytes = result.len(),
      "tool call returned"
    );
    Ok(result)
  }

  fn list_resources(
    &mut self,
    self_: Resource<ToolingEntry>,
  ) -> Result<Vec<tooling_bindings::ResourceInfo>, String> {
    let entry = self.table.get(&self_).map_err(|e| e.to_string())?;
    let tooling = Arc::clone(&entry.tooling);
    let rt = Arc::clone(&self.ctx.rt());
    rt.block_on(tooling.list_resources())
      .map(|resources| resources.into_iter().map(ResourceInfo::into).collect())
      .map_err(|e| e.to_string())
  }

  fn subscribe_resource_list(
    &mut self,
    self_: Resource<ToolingEntry>,
  ) -> Result<String, String> {
    let entry = self.table.get(&self_).map_err(|e| e.to_string())?;
    let tooling = Arc::clone(&entry.tooling);
    let rt = Arc::clone(&self.ctx.rt());
    let stream = rt
      .block_on(tooling.subscribe_resource_list())
      .map_err(|e| e.to_string())?;
    let uuid = crate::host::bus::new_uuid();
    tracing::debug!(
      agent = %self.ctx.name,
      tooling = %entry.name,
      uuid = %uuid,
      "subscribing to the resource list"
    );
    crate::host::resources::spawn_pump(
      Arc::clone(&self.ctx.resources),
      rt,
      Arc::clone(&self.ctx.bus),
      self.ctx.name.clone(),
      uuid.clone(),
      tooling,
      stream,
    );
    Ok(uuid)
  }

  fn subscribe_resource(
    &mut self,
    self_: Resource<ToolingEntry>,
    uri: String,
  ) -> Result<String, String> {
    let entry = self.table.get(&self_).map_err(|e| e.to_string())?;
    let tooling = Arc::clone(&entry.tooling);
    let rt = Arc::clone(&self.ctx.rt());
    let stream = rt
      .block_on(tooling.subscribe_resource(&uri))
      .map_err(|e| e.to_string())?;
    let uuid = crate::host::bus::new_uuid();
    tracing::debug!(
      agent = %self.ctx.name,
      tooling = %entry.name,
      uri = %uri,
      uuid = %uuid,
      "subscribing to a resource"
    );
    crate::host::resources::spawn_pump(
      Arc::clone(&self.ctx.resources),
      rt,
      Arc::clone(&self.ctx.bus),
      self.ctx.name.clone(),
      uuid.clone(),
      tooling,
      stream,
    );
    Ok(uuid)
  }

  fn unsubscribe_resource_list(
    &mut self,
    self_: Resource<ToolingEntry>,
    uuid: String,
  ) {
    let _ = self_;
    self.ctx.resources.cancel(&uuid);
    tracing::debug!(
      agent = %self.ctx.name,
      uuid = %uuid,
      "cancelling a resource-list subscription"
    );
  }

  fn unsubscribe_resource(
    &mut self,
    self_: Resource<ToolingEntry>,
    uuid: String,
  ) {
    let _ = self_;
    self.ctx.resources.cancel(&uuid);
    tracing::debug!(
      agent = %self.ctx.name,
      uuid = %uuid,
      "cancelling a resource subscription"
    );
  }

  fn drop(&mut self, self_: Resource<ToolingEntry>) -> wasmtime::Result<()> {
    self.table.delete(self_)?;
    Ok(())
  }
}

impl host_bindings::Host for Host {
  fn log(&mut self, level: String, message: String) {
    let agent = self.ctx.name.clone();
    match level.as_str() {
      "trace" => tracing::trace!(agent = %agent, message),
      "debug" => tracing::debug!(agent = %agent, message),
      "warn" => tracing::warn!(agent = %agent, message),
      "error" => tracing::error!(agent = %agent, message),
      _ => tracing::info!(agent = %agent, message),
    }
  }

  fn now(&mut self) -> u64 {
    crate::host::time::now_ticks()
  }

  fn timestamp_add(&mut self, ts: u64, ms: u64) -> u64 {
    crate::host::time::add(ts, ms)
  }

  fn timestamp_sub(&mut self, ts: u64, ms: u64) -> u64 {
    crate::host::time::sub(ts, ms)
  }

  fn timestamp_diff(&mut self, a: u64, b: u64) -> i64 {
    crate::host::time::diff(a, b)
  }

  fn timestamp_format(&mut self, ts: u64, format: String) -> String {
    crate::host::time::format(ts, &format)
  }

  fn wait_timestamp(&mut self, ts: u64) -> Result<String, String> {
    let uuid = crate::host::bus::new_uuid();
    crate::host::time::wait_timestamp(
      &self.ctx.bus,
      &self.ctx.rt(),
      &self.ctx.timers,
      &self.ctx.name,
      &uuid,
      ts,
    )?;
    Ok(uuid)
  }

  fn wait_duration(&mut self, ms: u64) -> Result<String, String> {
    let uuid = crate::host::bus::new_uuid();
    crate::host::time::wait_duration(
      &self.ctx.bus,
      &self.ctx.rt(),
      &self.ctx.timers,
      &self.ctx.name,
      &uuid,
      ms,
    );
    Ok(uuid)
  }

  fn wait_cron(&mut self, spec: String) -> Result<String, String> {
    let uuid = crate::host::bus::new_uuid();
    crate::host::time::wait_cron(
      &self.ctx.bus,
      &self.ctx.rt(),
      &self.ctx.timers,
      &self.ctx.name,
      &uuid,
      &spec,
    )?;
    Ok(uuid)
  }

  fn send(&mut self, agent: String, payload: String) {
    tracing::debug!(caller = %self.ctx.name, dest = %agent, "host send");
    self.ctx.bus.send(&self.ctx.name, &agent, payload);
  }

  fn subscribe(&mut self, agent: String) -> Result<String, String> {
    let uuid = self.ctx.bus.subscribe(&self.ctx.name, &agent);
    tracing::info!(agent = %self.ctx.name, source = %agent, uuid = %uuid, "host subscribe");
    Ok(uuid)
  }

  fn unsubscribe(&mut self, uuid: String) {
    let removed = self.ctx.bus.unsubscribe(&self.ctx.name, &uuid);
    tracing::debug!(
      agent = %self.ctx.name,
      uuid = %uuid,
      removed,
      "host unsubscribe"
    );
  }

  fn cancel(&mut self, uuid: String) {
    self.ctx.timers.cancel(&uuid);
    tracing::debug!(agent = %self.ctx.name, uuid = %uuid, "host cancel");
  }

  fn recv(&mut self) -> Result<host_bindings::EventEnvelope, String> {
    tracing::debug!(agent = %self.ctx.name, "host recv waiting for an event");
    let envelope = self
      .ctx
      .bus
      .recv(&self.ctx.name, Duration::from_secs(60))
      .map_err(|e| e.to_string())?;
    Ok(EventEnvelope {
      id: envelope.id,
      event: out_event(envelope.event),
    })
  }

  fn try_recv(
    &mut self,
  ) -> Result<Option<host_bindings::EventEnvelope>, String> {
    tracing::trace!(agent = %self.ctx.name, "host try_recv poll");
    Ok(
      self
        .ctx
        .bus
        .try_recv(&self.ctx.name)
        .map_err(|e| e.to_string())?
        .map(|envelope| EventEnvelope {
          id: envelope.id,
          event: out_event(envelope.event),
        }),
    )
  }

  fn new_uuid(&mut self) -> String {
    crate::host::bus::new_uuid()
  }
}

type EventEnvelope = host_bindings::EventEnvelope;

fn out_event(event: Event) -> types_bindings::Event {
  match event {
    Event::Message(payload) => types_bindings::Event::Message(payload),
    Event::Error(message) => types_bindings::Event::Error(message),
    Event::Timer => types_bindings::Event::Timer,
    Event::ChatDelta(d) => types_bindings::Event::ChatDelta(out_msg(d)),
    Event::StreamEnd => types_bindings::Event::StreamEnd,
    Event::ResourceListUpdated(resources) => {
      types_bindings::Event::ResourceListUpdated(
        resources.into_iter().map(ResourceInfo::into).collect(),
      )
    }
    Event::ResourceUpdated(content) => {
      types_bindings::Event::ResourceUpdated(content.into())
    }
  }
}

fn in_msg(m: provider_bindings::ChatMessage) -> ChatMessage {
  ChatMessage {
    role: match m.role {
      provider_bindings::Role::System => Role::System,
      provider_bindings::Role::User => Role::User,
      provider_bindings::Role::Assistant => Role::Assistant,
      provider_bindings::Role::Tool => Role::Tool,
    },
    content: m.content,
    tool_call: m.tool_call.map(|tc| ToolCall {
      id: tc.id,
      name: tc.name,
      arguments: tc.arguments,
    }),
  }
}

fn in_tool(t: tooling_bindings::Tool) -> Tool {
  Tool {
    name: t.name,
    description: t.description,
    input_schema: serde_json::from_str(&t.input_schema)
      .unwrap_or(serde_json::Value::Null),
  }
}

fn out_msg(d: ChatDelta) -> types_bindings::ChatDelta {
  types_bindings::ChatDelta {
    content: d.content,
    tool_call: d.tool_call.map(|tc| types_bindings::ToolCall {
      id: tc.id,
      name: tc.name,
      arguments: tc.arguments,
    }),
    finish_reason: d.finish_reason,
  }
}

impl From<crate::tooling::Tool> for tooling_bindings::Tool {
  fn from(t: crate::tooling::Tool) -> Self {
    Self {
      name: t.name,
      description: t.description,
      input_schema: serde_json::to_string(&t.input_schema)
        .unwrap_or_else(|_| "null".into()),
    }
  }
}

impl From<crate::tooling::ResourceInfo> for tooling_bindings::ResourceInfo {
  fn from(r: crate::tooling::ResourceInfo) -> Self {
    Self {
      uri: r.uri,
      name: r.name,
      description: r.description,
      mime_type: r.mime_type,
    }
  }
}

impl From<ResourceContent> for types_bindings::ResourceContent {
  fn from(c: ResourceContent) -> Self {
    Self {
      uri: c.uri,
      mime_type: c.mime_type,
      content: c.content,
    }
  }
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;
  use std::path::PathBuf;

  use super::*;
  use crate::bindings::omw::omw::host::Host as _;
  use crate::host::bus::MessageBus;
  use crate::host::streams::StreamRegistry;

  fn test_host() -> anyhow::Result<Host> {
    let bus = Arc::new(MessageBus::new());
    let ctx = AgentContext::new(
      "test-agent".to_string(),
      PathBuf::from("unused.rhai"),
      HashMap::new(),
      HashMap::new(),
      bus,
      Arc::new(StreamRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
    )?;
    Ok(Host {
      ctx,
      table: Default::default(),
      wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
    })
  }

  fn in_msg_with(role: provider_bindings::Role) -> ChatMessage {
    in_msg(provider_bindings::ChatMessage {
      role,
      content: Some("hi".to_string()),
      tool_call: None,
    })
  }

  #[test]
  fn new_uuid_is_a_valid_v4() -> anyhow::Result<()> {
    let mut host = test_host()?;
    let uuid = host.new_uuid();
    let parsed = uuid::Uuid::parse_str(&uuid)
      .map_err(|_| anyhow::anyhow!("not a valid uuid: {uuid:?}"))?;
    assert_eq!(parsed.get_version(), Some(uuid::Version::Random));
    Ok(())
  }

  #[test]
  fn in_msg_maps_each_role() {
    let system = in_msg_with(provider_bindings::Role::System);
    assert_eq!(system.role, Role::System);
    let user = in_msg_with(provider_bindings::Role::User);
    assert_eq!(user.role, Role::User);
    let assistant = in_msg_with(provider_bindings::Role::Assistant);
    assert_eq!(assistant.role, Role::Assistant);
    let tool = in_msg_with(provider_bindings::Role::Tool);
    assert_eq!(tool.role, Role::Tool);
    assert_eq!(user.content.as_deref(), Some("hi"));
    assert!(user.tool_call.is_none());
  }

  #[test]
  fn in_msg_roundtrips_tool_call() -> anyhow::Result<()> {
    let wire_tc = provider_bindings::ToolCall {
      id: "call_1".to_string(),
      name: "get_weather".to_string(),
      arguments: "{}".to_string(),
    };
    let out = in_msg(provider_bindings::ChatMessage {
      role: provider_bindings::Role::Assistant,
      content: None,
      tool_call: Some(wire_tc),
    });
    let tc = out
      .tool_call
      .ok_or_else(|| anyhow::anyhow!("missing tool call"))?;
    assert_eq!(tc.id, "call_1");
    assert_eq!(tc.name, "get_weather");
    assert_eq!(tc.arguments, "{}");
    Ok(())
  }

  #[test]
  fn out_msg_roundtrips_delta() {
    let delta = ChatDelta {
      content: Some("x".to_string()),
      tool_call: None,
      finish_reason: Some("stop".to_string()),
    };
    let wire = out_msg(delta);
    assert_eq!(wire.content.as_deref(), Some("x"));
    assert_eq!(wire.finish_reason.as_deref(), Some("stop"));
    assert!(wire.tool_call.is_none());
  }

  #[test]
  fn in_tool_parses_input_schema_and_handles_malformed() {
    let malformed = tooling_bindings::Tool {
      name: "t".to_string(),
      description: None,
      input_schema: "not json".to_string(),
    };
    let out = in_tool(malformed);
    assert_eq!(out.input_schema, serde_json::Value::Null);

    let valid = tooling_bindings::Tool {
      name: "t".to_string(),
      description: Some("does things".to_string()),
      input_schema: r#"{"type":"object"}"#.to_string(),
    };
    let out = in_tool(valid);
    assert_eq!(out.input_schema, serde_json::json!({ "type": "object" }));
    assert_eq!(out.description.as_deref(), Some("does things"));
  }

  #[test]
  fn tool_into_wire_serializes_input_schema() {
    let tool = Tool {
      name: "t".to_string(),
      description: None,
      input_schema: serde_json::json!({ "type": "object" }),
    };
    let wire: tooling_bindings::Tool = tool.into();
    assert_eq!(wire.input_schema, r#"{"type":"object"}"#);

    let null = Tool {
      name: "n".to_string(),
      description: None,
      input_schema: serde_json::Value::Null,
    };
    let wire: tooling_bindings::Tool = null.into();
    assert_eq!(wire.input_schema, "null");
  }
}
