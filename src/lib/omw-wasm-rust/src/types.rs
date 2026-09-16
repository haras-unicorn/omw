//! Builders and accessors over the generated `types`.
//!
//! These never hide fields: every record stays fully constructible, so
//! additive WIT changes keep compiling against the raw structs.

use crate::omw::omw::types::{
  ChatDelta, ChatMessage, ChatResult, Event, EventEnvelope, Tool, ToolCall,
};

impl crate::omw::omw::types::Role {
  /// The role as the lowercase string the docs use.
  pub fn as_str(&self) -> &'static str {
    use crate::omw::omw::types::Role;
    match self {
      Role::System => "system",
      Role::User => "user",
      Role::Assistant => "assistant",
      Role::Tool => "tool",
    }
  }
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
      role: crate::omw::omw::types::Role::User,
      content: Some(content.into()),
      tool_call: None,
    }
  }

  /// A system prompt message.
  pub fn system(content: impl Into<String>) -> Self {
    Self {
      role: crate::omw::omw::types::Role::System,
      content: Some(content.into()),
      tool_call: None,
    }
  }

  /// An assistant message with text content.
  pub fn assistant(content: impl Into<String>) -> Self {
    Self {
      role: crate::omw::omw::types::Role::Assistant,
      content: Some(content.into()),
      tool_call: None,
    }
  }

  /// Attach a tool call to this message.
  pub fn with_tool_call(mut self, call: ToolCall) -> Self {
    self.tool_call = Some(call);
    self
  }

  /// Replace the text content of this message.
  pub fn with_content(mut self, content: impl Into<String>) -> Self {
    self.content = Some(content.into());
    self
  }
}

impl ChatDelta {
  /// A delta carrying a text chunk.
  pub fn text(content: impl Into<String>) -> Self {
    Self {
      content: Some(content.into()),
      tool_call: None,
      finish_reason: None,
    }
  }

  /// A terminal delta: ends the stream/session it is pushed to.
  pub fn finish(reason: impl Into<String>) -> Self {
    Self {
      content: None,
      tool_call: None,
      finish_reason: Some(reason.into()),
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
}

impl Tool {
  /// A callable tool with a JSON-schema `input_schema` document.
  pub fn new(name: impl Into<String>, input_schema: impl Into<String>) -> Self {
    Self {
      name: name.into(),
      description: None,
      input_schema: input_schema.into(),
    }
  }

  /// Attach a human-readable description.
  pub fn with_description(mut self, desc: impl Into<String>) -> Self {
    self.description = Some(desc.into());
    self
  }
}

impl ChatResult {
  /// The concatenated text, absent on tool-call-only responses.
  pub fn text(&self) -> Option<&str> {
    self.content.as_deref()
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

#[cfg(test)]
mod tests {
  use super::*;
  use crate::omw::omw::types::Role;

  #[test]
  fn role_names_match_docs() {
    assert_eq!(Role::System.as_str(), "system");
    assert_eq!(Role::User.as_str(), "user");
    assert_eq!(Role::Assistant.as_str(), "assistant");
    assert_eq!(Role::Tool.as_str(), "tool");
  }

  #[test]
  fn message_builders_set_role_and_content() {
    let msg = ChatMessage::user("hi");
    assert!(matches!(msg.role, Role::User));
    assert_eq!(msg.content.as_deref(), Some("hi"));
    assert!(msg.tool_call.is_none());

    let call = ToolCall::new("1", "search", "{}");
    let msg = ChatMessage::system("be nice").with_tool_call(call);
    assert!(matches!(msg.role, Role::System));
    assert_eq!(msg.tool_call.map(|c| c.name), Some("search".to_string()));

    let msg = ChatMessage::assistant("hello").with_content("bye");
    assert_eq!(msg.content.as_deref(), Some("bye"));
  }

  #[test]
  fn delta_finish_is_terminal() {
    let delta = ChatDelta::text("chunk");
    assert!(!delta.is_terminal());
    assert_eq!(delta.text_content(), Some("chunk"));

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
      tool_calls: Vec::new(),
      finish_reason: Some("stop".to_string()),
    };
    assert_eq!(result.text(), Some("answer"));
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
