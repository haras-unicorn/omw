//! Typed handle over a configured provider instance.
//!
//! Looks the instance up once with `get`; further calls go through the
//! handle so callers never repeat the name.

use crate::omw::omw::provider as raw;
use crate::omw::omw::types::{ChatMessage, ChatResult, Tool};

/// One configured provider instance, looked up by name.
pub struct Provider {
  inner: raw::Provider,
}

impl Provider {
  /// Look up a configured provider instance by name.
  pub fn get(name: &str) -> Result<Self, String> {
    raw::get(name).map(|inner| Self { inner })
  }

  /// The configured name of this instance.
  pub fn name(&self) -> String {
    self.inner.name()
  }

  /// Which implementation this is.
  pub fn kind(&self) -> String {
    self.inner.kind()
  }

  /// Model names this provider exposes.
  pub fn list_models(&self) -> Vec<String> {
    self.inner.list_models()
  }

  /// Run a chat conversation to completion, in-band.
  pub fn chat(
    &self,
    model: &str,
    messages: &[ChatMessage],
    tools: &[Tool],
  ) -> Result<ChatResult, String> {
    self.inner.chat(model, messages, tools)
  }

  /// Open a streaming chat response. Deltas arrive in the inbox as
  /// `chat-delta` events until `chat-end`; dropping the guard cancels.
  pub fn chat_stream(
    &self,
    model: &str,
    messages: &[ChatMessage],
    tools: &[Tool],
  ) -> Result<StreamGuard, String> {
    let name = self.inner.name();
    let uuid = self.inner.chat_stream(model, messages, tools)?;
    Ok(StreamGuard {
      provider: name,
      uuid,
    })
  }

  /// Whether a chat stream is still open.
  pub fn is_open(&self, uuid: &str) -> bool {
    self.inner.is_open(uuid)
  }

  /// Cancel an open stream by UUID.
  pub fn cancel(&self, uuid: &str) {
    self.inner.cancel(uuid);
  }
}

/// An open chat stream; cancels on drop.
pub struct StreamGuard {
  provider: String,
  uuid: String,
}

impl StreamGuard {
  /// The UUID handle tagging this stream's inbox events.
  pub fn uuid(&self) -> &str {
    &self.uuid
  }

  /// Whether the stream is still open.
  pub fn is_open(&self) -> bool {
    raw::get(&self.provider).is_ok_and(|p| p.is_open(&self.uuid))
  }

  /// Cancel the stream, dropping its pending delivery.
  pub fn cancel(&self) {
    if let Ok(p) = raw::get(&self.provider) {
      p.cancel(&self.uuid);
    }
  }
}

impl Drop for StreamGuard {
  fn drop(&mut self) {
    self.cancel();
  }
}
