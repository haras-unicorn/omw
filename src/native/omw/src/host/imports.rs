//! Host implementations of the `omw` import interfaces, bridging the
//! synchronous wasm host calls to the async provider/tooling implementations
//! and the message bus.
//!
//! Because the wasm engine runs synchronously here, async work is bridged two
//! ways:
//!   * `provider.chat-stream` spawns a pump task (see `host/streams.rs`) on the
//!     shared tokio runtime that delivers `chat-delta` / `chat-end` events into the
//!     agent's inbox.
//!   * `tooling.*` and `host.*` results are obtained with
//!     [`Runtime::block_on`], which is only legal on threads that are not
//!     themselves inside a tokio runtime (i.e. our `spawn_blocking` wasm
//!     thread).

use std::sync::Arc;

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

  fn list_models(&mut self, self_: Resource<ProviderEntry>) -> Vec<String> {
    let Some(entry) = self.table.get(&self_).ok() else {
      return Vec::new();
    };
    let entry_name = entry.name.clone();
    let agent = self.ctx.name.clone();
    let provider = Arc::clone(&entry.provider);
    let list = async move { provider.list_models().await };
    match self.ctx.block_on_reload(list) {
      Ok(models) => models,
      Err(error) => {
        tracing::warn!(agent = %agent, provider = %entry_name, error = %error, "provider models aborted");
        Vec::new()
      }
    }
  }

  fn chat(
    &mut self,
    self_: Resource<ProviderEntry>,
    model: String,
    messages: Vec<provider_bindings::ChatMessage>,
    tools: Vec<tooling_bindings::Tool>,
  ) -> Result<types_bindings::ChatResult, String> {
    let entry = self.table.get(&self_).map_err(|e| e.to_string())?;
    let entry_name = entry.name.clone();
    let agent = self.ctx.name.clone();
    let msgs: Vec<ChatMessage> = messages.into_iter().map(in_msg).collect();
    let tools: Vec<Tool> = tools.into_iter().map(in_tool).collect();
    let provider = Arc::clone(&entry.provider);
    tracing::debug!(
      agent = %agent,
      provider = %entry_name,
      "running a blocking chat"
    );
    let chat = async move { provider.chat(&model, msgs, tools).await };
    let result = self.ctx.block_on_reload(chat)?.map_err(|e| e.to_string())?;
    Ok(out_chat_result(result))
  }

  fn chat_stream(
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
    let tooling_name = entry.name.clone();
    let agent = self.ctx.name.clone();
    tracing::debug!(agent = %agent, tooling = %tooling_name, "listing tools");
    let list = async move { tooling.list_tools().await };
    let tools = self.ctx.block_on_reload(list)?.map_err(|e| e.to_string())?;
    tracing::debug!(agent = %agent, tooling = %tooling_name, count = tools.len(), "listed tools");
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
    let tooling_name = entry.name.clone();
    let agent = self.ctx.name.clone();
    let args =
      serde_json::from_str(&arguments).unwrap_or(serde_json::Value::Null);
    tracing::trace!(
      agent = %agent,
      tooling = %tooling_name,
      tool = %name,
      arg_bytes = arguments.len(),
      "queuing a tool call"
    );
    let uuid = crate::host::bus::new_uuid();
    crate::host::tool_calls::spawn_pump(
      tooling,
      self.ctx.rt(),
      Arc::clone(&self.ctx.bus),
      Arc::clone(&self.ctx.tool_calls),
      agent,
      uuid.clone(),
      name,
      args,
    );
    Ok(uuid)
  }

  fn is_open(&mut self, _self_: Resource<ToolingEntry>, uuid: String) -> bool {
    self.ctx.tool_calls.is_open(&uuid)
  }

  fn cancel(&mut self, _self_: Resource<ToolingEntry>, uuid: String) {
    self.ctx.tool_calls.cancel(&uuid);
    tracing::debug!(agent = %self.ctx.name, uuid = %uuid, "cancelling a tool call");
  }

  fn call_tool_blocking(
    &mut self,
    self_: Resource<ToolingEntry>,
    tool: String,
    arguments: String,
  ) -> Result<tooling_bindings::ToolResult, String> {
    let entry = self.table.get(&self_).map_err(|e| e.to_string())?;
    let tooling = Arc::clone(&entry.tooling);
    let tooling_name = entry.name.clone();
    let agent = self.ctx.name.clone();
    let args =
      serde_json::from_str(&arguments).unwrap_or(serde_json::Value::Null);
    tracing::trace!(
      agent = %agent,
      tooling = %tooling_name,
      tool = %tool,
      arg_bytes = arguments.len(),
      "calling a tool"
    );
    let tool_for_call = tool.clone();
    let call = async move { tooling.call_tool(&tool_for_call, args).await };
    let result = self.ctx.block_on_reload(call)?.map_err(|e| e.to_string())?;
    tracing::trace!(
      agent = %agent,
      tooling = %tooling_name,
      result_bytes = result.len(),
      "tool call returned"
    );
    Ok(tooling_bindings::ToolResult {
      name: tool,
      arguments,
      value: result,
    })
  }

  fn list_resources(
    &mut self,
    self_: Resource<ToolingEntry>,
  ) -> Result<Vec<tooling_bindings::ResourceInfo>, String> {
    let entry = self.table.get(&self_).map_err(|e| e.to_string())?;
    let tooling = Arc::clone(&entry.tooling);
    let list = async move { tooling.list_resources().await };
    self
      .ctx
      .block_on_reload(list)?
      .map(|resources| resources.into_iter().map(ResourceInfo::into).collect())
      .map_err(|e| e.to_string())
  }

  fn read_resource(
    &mut self,
    self_: Resource<ToolingEntry>,
    uri: String,
  ) -> Result<tooling_bindings::ResourceContent, String> {
    let entry = self.table.get(&self_).map_err(|e| e.to_string())?;
    let tooling = Arc::clone(&entry.tooling);
    let read = async move { tooling.read_resource(&uri).await };
    self
      .ctx
      .block_on_reload(read)?
      .map(tooling_bindings::ResourceContent::from)
      .map_err(|e| e.to_string())
  }

  fn subscribe_resource_list(
    &mut self,
    self_: Resource<ToolingEntry>,
  ) -> Result<String, String> {
    let entry = self.table.get(&self_).map_err(|e| e.to_string())?;
    let tooling = Arc::clone(&entry.tooling);
    let tooling_name = entry.name.clone();
    let agent = self.ctx.name.clone();
    let tooling_for_sub = Arc::clone(&tooling);
    let subscribe =
      async move { tooling_for_sub.subscribe_resource_list().await };
    let stream = self
      .ctx
      .block_on_reload(subscribe)?
      .map_err(|e| e.to_string())?;
    let uuid = crate::host::bus::new_uuid();
    tracing::debug!(
      agent = %agent,
      tooling = %tooling_name,
      uuid = %uuid,
      "subscribing to the resource list"
    );
    crate::host::resources::spawn_pump(
      Arc::clone(&self.ctx.resources),
      self.ctx.rt(),
      Arc::clone(&self.ctx.bus),
      agent,
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
    let tooling_name = entry.name.clone();
    let agent = self.ctx.name.clone();
    let tooling_for_sub = Arc::clone(&tooling);
    let uri_for_sub = uri.clone();
    let subscribe =
      async move { tooling_for_sub.subscribe_resource(&uri_for_sub).await };
    let stream = self
      .ctx
      .block_on_reload(subscribe)?
      .map_err(|e| e.to_string())?;
    let uuid = crate::host::bus::new_uuid();
    tracing::debug!(
      agent = %agent,
      tooling = %tooling_name,
      uri = %uri,
      uuid = %uuid,
      "subscribing to a resource"
    );
    crate::host::resources::spawn_pump(
      Arc::clone(&self.ctx.resources),
      self.ctx.rt(),
      Arc::clone(&self.ctx.bus),
      agent,
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

  fn time_now(&mut self) -> u64 {
    crate::host::time::now_ticks()
  }

  fn time_format(&mut self, ts: u64, format: String) -> String {
    crate::host::time::format(ts, &format)
  }

  fn wait_until(&mut self, ts: u64) -> Result<String, String> {
    let uuid = crate::host::bus::new_uuid();
    crate::host::time::wait_until(
      &self.ctx.bus,
      &self.ctx.rt(),
      &self.ctx.timers,
      &self.ctx.name,
      &uuid,
      ts,
    )?;
    Ok(uuid)
  }

  fn wait_for(&mut self, ms: u64) -> Result<String, String> {
    let uuid = crate::host::bus::new_uuid();
    crate::host::time::wait_for(
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

  fn send_agent(&mut self, agent: String, payload: String) {
    tracing::debug!(caller = %self.ctx.name, dest = %agent, "host send-agent");
    self.ctx.bus.send(&self.ctx.name, &agent, payload);
  }

  fn subscribe_agent(&mut self, agent: String) -> Result<String, String> {
    let uuid = self.ctx.bus.subscribe(&self.ctx.name, &agent);
    tracing::info!(agent = %self.ctx.name, source = %agent, uuid = %uuid, "host subscribe-agent");
    Ok(uuid)
  }

  fn subscribe_lifecycle(&mut self) -> Result<String, String> {
    let result = self.ctx.bus.lifecycle_subscribe(&self.ctx.name);
    match &result {
      Ok(uuid) => {
        tracing::info!(agent = %self.ctx.name, uuid = %uuid, "host subscribe-lifecycle")
      }
      Err(error) => {
        tracing::warn!(agent = %self.ctx.name, error = %error, "host subscribe-lifecycle rejected")
      }
    }
    result
  }

  fn unsubscribe_lifecycle(&mut self, uuid: String) {
    let removed = self.ctx.bus.lifecycle_unsubscribe(&self.ctx.name, &uuid);
    tracing::debug!(
      agent = %self.ctx.name,
      uuid = %uuid,
      removed,
      "host unsubscribe-lifecycle"
    );
  }

  fn unsubscribe_agent(&mut self, uuid: String) {
    let removed = self.ctx.bus.unsubscribe(&self.ctx.name, &uuid);
    tracing::debug!(
      agent = %self.ctx.name,
      uuid = %uuid,
      removed,
      "host unsubscribe-agent"
    );
  }

  fn subscribe_endpoint(&mut self, model: String) -> Result<String, String> {
    if self.ctx.endpoint.is_none() {
      tracing::warn!(
        agent = %self.ctx.name,
        "host subscribe-endpoint rejected: the endpoint is not configured"
      );
      return Err("the endpoint is not configured".to_string());
    }
    let result = self.ctx.bus.endpoint_subscribe(&self.ctx.name, model);
    match &result {
      Ok(uuid) => tracing::info!(
        agent = %self.ctx.name,
        uuid = %uuid,
        "host subscribe-endpoint"
      ),
      Err(error) => tracing::warn!(
        agent = %self.ctx.name,
        error = %error,
        "host subscribe-endpoint rejected"
      ),
    }
    result
  }

  fn unsubscribe_endpoint(&mut self, uuid: String) {
    let model = self.ctx.bus.endpoint_unsubscribe(&self.ctx.name, &uuid);
    tracing::info!(
      agent = %self.ctx.name,
      uuid = %uuid,
      model = ?model.as_deref(),
      "host unsubscribe-endpoint"
    );
    if model.is_some()
      && let Some(registry) = &self.ctx.endpoint
    {
      registry.cancel_subscription(&uuid);
    }
  }

  fn stream_endpoint(
    &mut self,
    session: String,
    delta: types_bindings::ChatDelta,
  ) -> Result<(), String> {
    let Some(registry) = &self.ctx.endpoint else {
      return Err("the endpoint is not configured".to_string());
    };
    tracing::debug!(
      agent = %self.ctx.name,
      session = %session,
      "host stream-endpoint"
    );
    registry.push(&self.ctx.name, &session, in_delta(delta))
  }

  fn cancel_timer(&mut self, uuid: String) {
    self.ctx.timers.cancel(&uuid);
    tracing::debug!(agent = %self.ctx.name,uuid = %uuid,"host cancel-timer");
  }

  fn sleep_for(&mut self, ms: u64) {
    tracing::debug!(agent = %self.ctx.name,ms,"host sleep-for");
    if let Err(error) = self
      .ctx
      .block_on_reload(crate::host::time::sleep_future(ms))
    {
      tracing::debug!(agent = %self.ctx.name, error = %error, "sleep-for aborted");
    }
  }

  fn sleep_until(&mut self, ts: u64) -> Result<(), String> {
    let delay = crate::host::time::delay_until(ts).map_err(|error| {
      tracing::warn!(agent = %self.ctx.name,error,ts,"host sleep-until rejected");
      error
    })?;
    self
      .ctx
      .block_on_reload(crate::host::time::sleep_future_ms(delay))
      .map_err(|error| {
        tracing::debug!(agent = %self.ctx.name, error = %error, "sleep-until aborted");
        error
      })
  }

  fn sleep_cron(&mut self, spec: String) -> Result<(), String> {
    let delay = crate::host::time::delay_cron(&spec).map_err(|error| {
      tracing::warn!(agent = %self.ctx.name,error,spec = %spec,"host sleep-cron rejected");
      error
    })?;
    self
      .ctx
      .block_on_reload(crate::host::time::sleep_future_ms(delay))
      .map_err(|error| {
        tracing::debug!(agent = %self.ctx.name, error = %error, "sleep-cron aborted");
        error
      })
  }

  fn recv(&mut self) -> Result<host_bindings::EventEnvelope, String> {
    tracing::debug!(agent = %self.ctx.name, "host recv waiting for an event");
    let flag = self.ctx.clone();
    let name = self.ctx.name.clone();
    let check = self.ctx.clone();
    let envelope = self
      .ctx
      .bus
      .recv_while(&name, self.ctx.tunables().recv_timeout(), move || {
        flag.reload_requested() || flag.shutdown_requested()
      })
      .map_err(|error| {
        if error.to_string() == "stop requested" {
          if check.shutdown_requested() {
            return "agent shutting down".to_string();
          }
          return "agent reloaded".to_string();
        }
        error.to_string()
      })?;
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

  fn base64_encode(&mut self, bytes: Vec<u8>) -> String {
    use base64::Engine as _;
    tracing::trace!(
      agent = %self.ctx.name,
      bytes = bytes.len(),
      "host base64-encode"
    );
    base64::prelude::BASE64_STANDARD.encode(bytes)
  }

  fn base64_decode(&mut self, data: String) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    let result = base64::prelude::BASE64_STANDARD
      .decode(data)
      .map_err(|e| e.to_string())?;
    tracing::trace!(
      agent = %self.ctx.name,
      bytes = result.len(),
      "host base64-decode"
    );
    Ok(result)
  }

  fn memory_get(&mut self, key: String) -> Option<String> {
    tracing::trace!(agent = %self.ctx.name, key = %key, "host memory-get");
    self.ctx.memory.get(&key)
  }

  fn memory_set(&mut self, key: String, value: String) {
    tracing::debug!(agent = %self.ctx.name, key = %key, "host memory-set");
    self.ctx.memory.set(key, value);
  }

  fn memory_remove(&mut self, key: String) -> bool {
    tracing::debug!(agent = %self.ctx.name, key = %key, "host memory-remove");
    self.ctx.memory.remove(&key)
  }
}

type EventEnvelope = host_bindings::EventEnvelope;

fn out_event(event: Event) -> types_bindings::Event {
  match event {
    Event::Message(payload) => types_bindings::Event::Message(payload),
    Event::Error(message) => types_bindings::Event::Error(message),
    Event::Timer => types_bindings::Event::Timer,
    Event::Reload => types_bindings::Event::Reload,
    Event::Shutdown => types_bindings::Event::Shutdown,
    Event::ChatDelta(d) => types_bindings::Event::ChatDelta(out_msg(d)),
    Event::ChatEnd => types_bindings::Event::ChatEnd,
    Event::ToolResult(r) => {
      types_bindings::Event::ToolResult(types_bindings::ToolResult {
        name: r.name,
        arguments: r.arguments,
        value: r.result,
      })
    }
    Event::ResourceListUpdated(resources) => {
      types_bindings::Event::ResourceListUpdated(
        resources.into_iter().map(ResourceInfo::into).collect(),
      )
    }
    Event::ResourceUpdated(content) => {
      types_bindings::Event::ResourceUpdated(content.into())
    }
    Event::EndpointMessage(m) => {
      types_bindings::Event::EndpointMessage(types_bindings::EndpointMessage {
        session: m.session,
        messages: m.messages.into_iter().map(out_chat_msg).collect(),
        tools: m.tools.into_iter().map(Tool::into).collect(),
      })
    }
    Event::EndpointSessionEnd(e) => types_bindings::Event::EndpointSessionEnd(
      types_bindings::EndpointSessionEnd {
        session: e.session,
        error: e.error,
      },
    ),
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

/// Map one host-side [`ChatMessage`] back onto the wire type used by the
/// endpoint-message event payload.
fn out_chat_msg(m: ChatMessage) -> provider_bindings::ChatMessage {
  provider_bindings::ChatMessage {
    role: match m.role {
      Role::System => provider_bindings::Role::System,
      Role::User => provider_bindings::Role::User,
      Role::Assistant => provider_bindings::Role::Assistant,
      Role::Tool => provider_bindings::Role::Tool,
    },
    content: m.content,
    tool_call: m.tool_call.map(|tc| provider_bindings::ToolCall {
      id: tc.id,
      name: tc.name,
      arguments: tc.arguments,
    }),
  }
}

/// Map one wire [`ChatDelta`] (from `host.stream-endpoint`) back onto the
/// host-side delta type.
fn in_delta(d: types_bindings::ChatDelta) -> ChatDelta {
  ChatDelta {
    content: d.content,
    tool_call: d.tool_call.map(|tc| ToolCall {
      id: tc.id,
      name: tc.name,
      arguments: tc.arguments,
    }),
    finish_reason: d.finish_reason,
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

fn out_chat_result(
  r: crate::provider::ChatResult,
) -> types_bindings::ChatResult {
  types_bindings::ChatResult {
    content: r.content,
    tool_calls: r.tool_calls.into_iter().map(out_tool_call).collect(),
    finish_reason: r.finish_reason,
  }
}

fn out_tool_call(tc: crate::provider::ToolCall) -> types_bindings::ToolCall {
  types_bindings::ToolCall {
    id: tc.id,
    name: tc.name,
    arguments: tc.arguments,
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
      Arc::new(crate::host::streams::CancelRegistry::new()),
      None,
    )?;
    Ok(Host {
      ctx,
      table: Default::default(),
      wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
    })
  }

  fn test_host_with_endpoint() -> anyhow::Result<Host> {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(crate::host::endpoint::EndpointRegistry::new(
      Arc::clone(&bus),
    ));
    bus
      .endpoint_subscribe("test-agent", "gpt-4o".to_string())
      .map_err(|e| anyhow::anyhow!(e))?;
    let ctx = AgentContext::new(
      "test-agent".to_string(),
      PathBuf::from("unused.rhai"),
      HashMap::new(),
      HashMap::new(),
      bus,
      Arc::new(StreamRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Arc::new(crate::host::streams::CancelRegistry::new()),
      Some(registry),
    )?;
    Ok(Host {
      ctx,
      table: Default::default(),
      wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
    })
  }

  #[test]
  fn stream_endpoint_errors_when_endpoint_not_configured() -> anyhow::Result<()>
  {
    let mut host = test_host()?;
    let delta = out_msg(ChatDelta {
      content: Some("hi".to_string()),
      tool_call: None,
      finish_reason: None,
    });
    let err = host
      .stream_endpoint("session-1".to_string(), delta)
      .unwrap_err();
    assert_eq!(err, "the endpoint is not configured");
    Ok(())
  }

  #[test]
  fn subscribe_endpoint_errors_when_endpoint_not_configured()
  -> anyhow::Result<()> {
    let mut host = test_host()?;
    let err = host.subscribe_endpoint("gpt-4o".to_string()).unwrap_err();
    assert_eq!(err, "the endpoint is not configured");
    Ok(())
  }

  #[tokio::test]
  async fn stream_endpoint_errors_when_receiver_closed() -> anyhow::Result<()> {
    let mut host = test_host_with_endpoint()?;
    let registry = host
      .ctx
      .endpoint
      .as_ref()
      .ok_or_else(|| anyhow::anyhow!("expected an endpoint registry"))?
      .clone();
    let mut open = registry.open("test-agent", "sub-1");
    open.rx.close();
    let err = host
      .stream_endpoint(
        open.session.clone(),
        out_msg(ChatDelta {
          content: Some("hi".to_string()),
          tool_call: None,
          finish_reason: None,
        }),
      )
      .unwrap_err();
    assert_eq!(err, "endpoint session receiver closed");
    Ok(())
  }

  #[tokio::test]
  async fn stream_endpoint_queues_non_terminal_deltas_and_rejects_after_termination()
  -> anyhow::Result<()> {
    let mut host = test_host_with_endpoint()?;
    let registry = host
      .ctx
      .endpoint
      .as_ref()
      .ok_or_else(|| anyhow::anyhow!("expected an endpoint registry"))?
      .clone();
    let mut open = registry.open("test-agent", "sub-1");
    host
      .stream_endpoint(
        open.session.clone(),
        out_msg(ChatDelta {
          content: Some("hi".to_string()),
          tool_call: None,
          finish_reason: None,
        }),
      )
      .map_err(|e| anyhow::anyhow!(e))?;
    match open.rx.recv().await {
      Some(crate::host::endpoint::Outbound::Delta(delta)) => {
        assert_eq!(delta.content.as_deref(), Some("hi"));
      }
      other => assert!(false, "unexpected outbound: {other:?}"),
    }
    host
      .stream_endpoint(
        open.session.clone(),
        out_msg(ChatDelta {
          content: None,
          tool_call: None,
          finish_reason: Some("stop".to_string()),
        }),
      )
      .map_err(|e| anyhow::anyhow!(e))?;
    match open.rx.recv().await {
      Some(crate::host::endpoint::Outbound::Delta(delta)) => {
        assert_eq!(delta.finish_reason.as_deref(), Some("stop"));
      }
      other => assert!(false, "unexpected outbound: {other:?}"),
    };
    assert_eq!(
      open.rx.recv().await,
      Some(crate::host::endpoint::Outbound::Close)
    );
    let err = host
      .stream_endpoint(
        open.session.clone(),
        out_msg(ChatDelta {
          content: None,
          tool_call: None,
          finish_reason: None,
        }),
      )
      .unwrap_err();
    assert_eq!(err, "unknown endpoint session");
    Ok(())
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
  fn base64_roundtrips_bytes() -> anyhow::Result<()> {
    let mut host = test_host()?;
    let encoded = host.base64_encode(vec![0, 1, 2, 255]);
    assert_eq!(encoded, "AAEC/w==");
    let decoded = host
      .base64_decode(encoded)
      .map_err(|e| anyhow::anyhow!(e))?;
    assert_eq!(decoded, vec![0, 1, 2, 255]);
    Ok(())
  }

  #[test]
  fn base64_empty_roundtrips() -> anyhow::Result<()> {
    let mut host = test_host()?;
    let encoded = host.base64_encode(Vec::new());
    assert_eq!(encoded, "");
    let decoded = host
      .base64_decode(encoded)
      .map_err(|e| anyhow::anyhow!(e))?;
    assert!(decoded.is_empty());
    Ok(())
  }

  #[test]
  fn base64_decode_rejects_invalid() -> anyhow::Result<()> {
    let mut host = test_host()?;
    assert!(host.base64_decode("!!!".to_string()).is_err());
    Ok(())
  }

  #[test]
  fn memory_get_set_remove_roundtrips() -> anyhow::Result<()> {
    let mut host = test_host()?;
    assert_eq!(host.memory_get("k".to_string()), None);
    assert!(!host.memory_remove("k".to_string()));
    host.memory_set("k".to_string(), "v".to_string());
    assert_eq!(host.memory_get("k".to_string()), Some("v".to_string()));
    host.memory_set("k".to_string(), "v2".to_string());
    assert_eq!(host.memory_get("k".to_string()), Some("v2".to_string()));
    assert!(host.memory_remove("k".to_string()));
    assert_eq!(host.memory_get("k".to_string()), None);
    assert!(!host.memory_remove("k".to_string()));
    Ok(())
  }

  #[test]
  fn memory_is_scoped_to_the_context() -> anyhow::Result<()> {
    let mut first = test_host()?;
    let mut second = test_host()?;
    first.memory_set("k".to_string(), "v".to_string());
    assert_eq!(second.memory_get("k".to_string()), None);
    Ok(())
  }

  #[test]
  fn memory_survives_a_context_clone_like_a_reload() -> anyhow::Result<()> {
    let mut host = test_host()?;
    host.memory_set("handle".to_string(), "uuid-1".to_string());
    let mut reloaded = Host {
      ctx: host.ctx.clone(),
      table: Default::default(),
      wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
    };
    assert_eq!(
      reloaded.memory_get("handle".to_string()),
      Some("uuid-1".to_string())
    );
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
