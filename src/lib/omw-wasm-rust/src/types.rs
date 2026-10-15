//! Builders and accessors over the generated `types`.
//!
//! These never hide fields: every record stays fully constructible, so
//! additive WIT changes keep compiling against the raw structs.

use crate::omw::omw::types::{
  ChatDelta, ChatMessage, ChatResult, EndpointMessage, Event, EventEnvelope,
  ResourceAnnotations, ResourceTemplate, Tool, ToolAnnotations, ToolCall,
  ToolCallDelta, ToolResult,
};
use serde::de::DeserializeOwned;

/// Parse one opaque JSON string field, surfacing the underlying error.
fn parse_json<T: DeserializeOwned>(raw: &str) -> Result<T, String> {
  serde_json::from_str(raw).map_err(|e| e.to_string())
}

impl ToolCall {
  /// A tool invocation: opaque JSON `arguments` for tool `name`.
  pub fn new(
    id: impl Into<String>,
    name: impl Into<String>,
    arguments: impl Into<String>,
  ) -> Self {
    Self {
      id: id.into(),
      name: name.into(),
      arguments: arguments.into(),
    }
  }
}

impl ChatMessage {
  /// A user message with text content.
  pub fn user(content: impl Into<String>) -> Self {
    Self {
      role: "user".to_string(),
      content: Some(content.into()),
      reasoning: None,
      tool_calls: Vec::new(),
      tool_call_id: None,
    }
  }

  /// A system prompt message.
  pub fn system(content: impl Into<String>) -> Self {
    Self {
      role: "system".to_string(),
      content: Some(content.into()),
      reasoning: None,
      tool_calls: Vec::new(),
      tool_call_id: None,
    }
  }

  /// An assistant message with text content.
  pub fn assistant(content: impl Into<String>) -> Self {
    Self {
      role: "assistant".to_string(),
      content: Some(content.into()),
      reasoning: None,
      tool_calls: Vec::new(),
      tool_call_id: None,
    }
  }

  /// Attach a tool call to this message.
  pub fn with_tool_call(mut self, call: ToolCall) -> Self {
    self.tool_calls.push(call);
    self
  }

  /// Set the identifier of the assistant tool call this message answers.
  pub fn with_tool_call_id(mut self, tool_call_id: impl Into<String>) -> Self {
    self.tool_call_id = Some(tool_call_id.into());
    self
  }

  /// Attach reasoning/thinking content to this message (sent back to the
  /// provider on assistant messages).
  pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
    self.reasoning = Some(reasoning.into());
    self
  }

  /// Replace the text content of this message.
  pub fn with_content(mut self, content: impl Into<String>) -> Self {
    self.content = Some(content.into());
    self
  }
}

impl ToolCallDelta {
  /// One tool-call fragment: `index` positions the call within the
  /// response's list, `id`/`name` appear on its first fragment, and
  /// `arguments` is a raw fragment of the opaque JSON string.
  pub fn new(
    index: u32,
    id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
  ) -> Self {
    Self {
      index,
      id,
      name,
      arguments,
    }
  }

  /// Attach the tool call identifier (present on the first fragment).
  pub fn with_id(mut self, id: impl Into<String>) -> Self {
    self.id = Some(id.into());
    self
  }

  /// Attach the tool name (present on the first fragment).
  pub fn with_name(mut self, name: impl Into<String>) -> Self {
    self.name = Some(name.into());
    self
  }

  /// Attach a raw fragment of the opaque JSON arguments string.
  pub fn with_arguments(mut self, arguments: impl Into<String>) -> Self {
    self.arguments = Some(arguments.into());
    self
  }
}

impl ChatDelta {
  /// A delta carrying a text chunk.
  pub fn text(content: impl Into<String>) -> Self {
    Self {
      content: Some(content.into()),
      reasoning: None,
      tool_calls: Vec::new(),
      finish_reason: None,
      usage: None,
    }
  }

  /// A delta carrying a reasoning/thinking chunk.
  pub fn reasoning(content: impl Into<String>) -> Self {
    Self {
      content: None,
      reasoning: Some(content.into()),
      tool_calls: Vec::new(),
      finish_reason: None,
      usage: None,
    }
  }

  /// A terminal delta: ends the stream/session it is pushed to.
  pub fn finish(reason: impl Into<String>) -> Self {
    Self {
      content: None,
      reasoning: None,
      tool_calls: Vec::new(),
      finish_reason: Some(reason.into()),
      usage: None,
    }
  }

  /// Whether this delta ends its stream/session.
  pub fn is_terminal(&self) -> bool {
    self.finish_reason.is_some()
  }

  /// The text chunk, if any.
  pub fn text_content(&self) -> Option<&str> {
    self.content.as_deref()
  }

  /// The reasoning/thinking chunk, if any.
  pub fn reasoning_content(&self) -> Option<&str> {
    self.reasoning.as_deref()
  }

  /// Parse the optional opaque JSON `usage` into `T`.
  pub fn usage_as<T: DeserializeOwned>(&self) -> Option<Result<T, String>> {
    self.usage.as_deref().map(parse_json)
  }
}

impl Tool {
  /// A callable tool with a JSON-schema `input_schema` document.
  pub fn new(name: impl Into<String>, input_schema: impl Into<String>) -> Self {
    Self {
      name: name.into(),
      title: None,
      description: None,
      input_schema: input_schema.into(),
      output_schema: None,
      annotations: None,
    }
  }

  /// Attach a human-readable display title.
  pub fn with_title(mut self, title: impl Into<String>) -> Self {
    self.title = Some(title.into());
    self
  }

  /// Attach a human-readable description.
  pub fn with_description(mut self, desc: impl Into<String>) -> Self {
    self.description = Some(desc.into());
    self
  }

  /// Attach a JSON-schema `output_schema` document describing the tool's
  /// structured output.
  pub fn with_output_schema(mut self, schema: impl Into<String>) -> Self {
    self.output_schema = Some(schema.into());
    self
  }

  /// Attach behavioral hints the tool declares about itself.
  pub fn with_annotations(mut self, annotations: ToolAnnotations) -> Self {
    self.annotations = Some(annotations);
    self
  }
}

impl ToolAnnotations {
  /// No hints declared; callers opt in with the builders.
  pub fn new() -> Self {
    Self {
      read_only_hint: None,
      destructive_hint: None,
      idempotent_hint: None,
      open_world_hint: None,
    }
  }

  /// Declare whether the tool modifies its environment.
  pub fn with_read_only_hint(mut self, hint: bool) -> Self {
    self.read_only_hint = Some(hint);
    self
  }

  /// Declare whether the tool may perform destructive updates.
  pub fn with_destructive_hint(mut self, hint: bool) -> Self {
    self.destructive_hint = Some(hint);
    self
  }

  /// Declare whether repeated calls with the same arguments have no
  /// additional effect.
  pub fn with_idempotent_hint(mut self, hint: bool) -> Self {
    self.idempotent_hint = Some(hint);
    self
  }

  /// Declare whether the tool may interact with entities outside its
  /// environment.
  pub fn with_open_world_hint(mut self, hint: bool) -> Self {
    self.open_world_hint = Some(hint);
    self
  }
}

impl Default for ToolAnnotations {
  fn default() -> Self {
    Self::new()
  }
}

impl ChatResult {
  /// The concatenated text, absent on tool-call-only responses.
  pub fn text(&self) -> Option<&str> {
    self.content.as_deref()
  }

  /// The concatenated reasoning/thinking content, when the provider produced
  /// any.
  pub fn reasoning(&self) -> Option<&str> {
    self.reasoning.as_deref()
  }

  /// Parse the optional opaque JSON `usage` into `T`.
  pub fn usage_as<T: DeserializeOwned>(&self) -> Option<Result<T, String>> {
    self.usage.as_deref().map(parse_json)
  }
}

impl Event {
  /// The event kind string used across the docs (`"chat-delta"`, …).
  pub fn kind(&self) -> &'static str {
    match self {
      Self::Message(_) => "message",
      Self::Error(_) => "error",
      Self::Timer => "timer",
      Self::Reload => "reload",
      Self::Shutdown => "shutdown",
      Self::ChatDelta(_) => "chat-delta",
      Self::ChatEnd => "chat-end",
      Self::ToolResult(_) => "tool-result",
      Self::ResourceListUpdated(_) => "resource-list-updated",
      Self::ResourceUpdated(_) => "resource-updated",
      Self::EndpointMessage(_) => "endpoint-message",
      Self::EndpointSessionEnd(_) => "endpoint-session-end",
    }
  }
}

impl EventEnvelope {
  /// The kind string of the wrapped event.
  pub fn kind(&self) -> &'static str {
    self.event.kind()
  }
}

impl ToolCall {
  /// Parse the tool call's opaque JSON `arguments` into `T`.
  pub fn arguments_as<T: DeserializeOwned>(&self) -> Result<T, String> {
    parse_json(&self.arguments)
  }
}

impl Tool {
  /// Parse the tool's JSON-schema `input_schema` into `T`.
  pub fn input_schema_as<T: DeserializeOwned>(&self) -> Result<T, String> {
    parse_json(&self.input_schema)
  }

  /// Parse the tool's optional JSON-schema `output_schema` into `T`.
  pub fn output_schema_as<T: DeserializeOwned>(
    &self,
  ) -> Option<Result<T, String>> {
    self.output_schema.as_deref().map(parse_json)
  }
}

impl ToolResult {
  /// Parse the tool's opaque JSON `arguments` into `T`.
  pub fn arguments_as<T: DeserializeOwned>(&self) -> Result<T, String> {
    parse_json(&self.arguments)
  }

  /// Parse the tool's `content` (an MCP content-block array) into `T`.
  pub fn content_as<T: DeserializeOwned>(&self) -> Result<T, String> {
    parse_json(&self.content)
  }

  /// Parse the tool's optional `structured_content` into `T`.
  pub fn structured_content_as<T: DeserializeOwned>(
    &self,
  ) -> Option<Result<T, String>> {
    self.structured_content.as_deref().map(parse_json)
  }
}

impl ResourceAnnotations {
  /// No annotations declared; callers opt in with the builders.
  pub fn new() -> Self {
    Self {
      audience: Vec::new(),
      priority: None,
      last_modified: None,
    }
  }

  /// Add one audience role this resource is intended for.
  pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
    self.audience.push(audience.into());
    self
  }

  /// Set the resource's relative importance in `[0, 1]`.
  pub fn with_priority(mut self, priority: f64) -> Self {
    self.priority = Some(priority);
    self
  }

  /// Set the resource's last-modified time as an ISO 8601 string.
  pub fn with_last_modified(
    mut self,
    last_modified: impl Into<String>,
  ) -> Self {
    self.last_modified = Some(last_modified.into());
    self
  }
}

impl Default for ResourceAnnotations {
  fn default() -> Self {
    Self::new()
  }
}

impl ResourceTemplate {
  /// A parameterized resource: an RFC 6570 `uri_template` and its `name`.
  pub fn new(uri_template: impl Into<String>, name: impl Into<String>) -> Self {
    Self {
      uri_template: uri_template.into(),
      name: name.into(),
      title: None,
      description: None,
      mime_type: None,
    }
  }

  /// Attach a human-readable display title.
  pub fn with_title(mut self, title: impl Into<String>) -> Self {
    self.title = Some(title.into());
    self
  }

  /// Attach a description of what the template is for.
  pub fn with_description(mut self, desc: impl Into<String>) -> Self {
    self.description = Some(desc.into());
    self
  }

  /// Attach the MIME type for resources matching this template.
  pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
    self.mime_type = Some(mime_type.into());
    self
  }
}

impl EndpointMessage {
  /// Parse the endpoint request's optional opaque JSON `params` into `T`.
  pub fn params_as<T: DeserializeOwned>(&self) -> Option<Result<T, String>> {
    self.params.as_deref().map(parse_json)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn tool_call_delta_builder_sets_fields() {
    let delta = ToolCallDelta::new(0, None, None, None)
      .with_id("1")
      .with_name("search")
      .with_arguments("{\"q\":");
    assert_eq!(delta.index, 0);
    assert_eq!(delta.id.as_deref(), Some("1"));
    assert_eq!(delta.name.as_deref(), Some("search"));
    assert_eq!(delta.arguments.as_deref(), Some("{\"q\":"));
  }

  #[test]
  fn message_builders_set_role_and_content() {
    let msg = ChatMessage::user("hi");
    assert_eq!(msg.role, "user");
    assert_eq!(msg.content.as_deref(), Some("hi"));
    assert!(msg.tool_calls.is_empty());
    assert!(msg.tool_call_id.is_none());

    let call = ToolCall::new("1", "search", "{}");
    let msg = ChatMessage::system("be nice").with_tool_call(call);
    assert_eq!(msg.role, "system");
    assert_eq!(
      msg.tool_calls.first().map(|c| c.name.clone()),
      Some("search".to_string())
    );

    let msg = ChatMessage::assistant("hello").with_content("bye");
    assert_eq!(msg.content.as_deref(), Some("bye"));

    let msg = ChatMessage::user("hi").with_tool_call_id("1");
    assert_eq!(msg.tool_call_id.as_deref(), Some("1"));
  }

  #[test]
  fn delta_finish_is_terminal() {
    let delta = ChatDelta::text("chunk");
    assert!(!delta.is_terminal());
    assert_eq!(delta.text_content(), Some("chunk"));
    assert!(delta.tool_calls.is_empty());

    let delta = ChatDelta::reasoning("thinking");
    assert!(!delta.is_terminal());
    assert_eq!(delta.reasoning_content(), Some("thinking"));

    let delta = ChatDelta::finish("stop");
    assert!(delta.is_terminal());
    assert_eq!(delta.finish_reason.as_deref(), Some("stop"));
  }

  #[test]
  fn tool_builder_defaults_empty_description() {
    let tool = Tool::new("search", "{}").with_description("searches");
    assert_eq!(tool.description.as_deref(), Some("searches"));
    assert_eq!(Tool::new("x", "{}").description, None);
  }

  #[test]
  fn chat_result_text_passthrough() {
    let result = ChatResult {
      content: Some("answer".to_string()),
      reasoning: Some("why".to_string()),
      tool_calls: Vec::new(),
      finish_reason: Some("stop".to_string()),
      usage: Some("{\"total_tokens\":5}".to_string()),
    };
    assert_eq!(result.text(), Some("answer"));
    assert_eq!(result.reasoning(), Some("why"));
    assert_eq!(
      result.usage_as::<serde_json::Value>().and_then(Result::ok),
      Some(serde_json::json!({ "total_tokens": 5 }))
    );
  }

  #[test]
  fn message_with_reasoning_sets_it() {
    let msg = ChatMessage::assistant("hi").with_reasoning("thought");
    assert_eq!(msg.reasoning.as_deref(), Some("thought"));
  }

  #[test]
  fn event_kinds_match_docs() {
    assert_eq!(Event::Timer.kind(), "timer");
    assert_eq!(Event::Reload.kind(), "reload");
    assert_eq!(Event::Shutdown.kind(), "shutdown");
    assert_eq!(Event::ChatEnd.kind(), "chat-end");
    assert_eq!(Event::Message("hi".to_string()).kind(), "message");
    assert_eq!(
      EventEnvelope {
        id: "id".to_string(),
        event: Event::Timer
      }
      .kind(),
      "timer"
    );
  }
}
