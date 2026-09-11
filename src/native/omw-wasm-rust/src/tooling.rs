//! Typed handle over a configured tooling instance plus RAII guards.

use crate::omw::omw::tooling as raw;
use crate::omw::omw::types::{ResourceContent, ResourceInfo, Tool, ToolResult};

/// One configured tooling instance, looked up by name.
pub struct Tooling {
  inner: raw::Tooling,
}

impl Tooling {
  /// Look up a configured tooling instance by name.
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

  /// Every tool this tooling exposes.
  pub fn list_tools(&self) -> Result<Vec<Tool>, String> {
    self.inner.list_tools()
  }

  /// Queue a tool call; the result arrives as a `tool-result` event.
  /// Dropping the guard cancels delivery.
  pub fn call_tool(
    &self,
    name: &str,
    arguments: &str,
  ) -> Result<CallGuard, String> {
    let tooling = self.inner.name();
    let uuid = self.inner.call_tool(name, arguments)?;
    Ok(CallGuard { tooling, uuid })
  }

  /// Invoke a tool, blocking until the result is ready.
  pub fn call_tool_blocking(
    &self,
    name: &str,
    arguments: &str,
  ) -> Result<ToolResult, String> {
    self.inner.call_tool_blocking(name, arguments)
  }

  /// Enumerate every resource this tooling exposes.
  pub fn list_resources(&self) -> Result<Vec<ResourceInfo>, String> {
    self.inner.list_resources()
  }

  /// Read one resource's current content.
  pub fn read_resource(&self, uri: &str) -> Result<ResourceContent, String> {
    self.inner.read_resource(uri)
  }

  /// Subscribe to the resource list changing. Dropping the guard
  /// unsubscribes.
  pub fn subscribe_resource_list(&self) -> Result<ResourceListGuard, String> {
    let tooling = self.inner.name();
    let uuid = self.inner.subscribe_resource_list()?;
    Ok(ResourceListGuard { tooling, uuid })
  }

  /// Subscribe to one resource's updates. Dropping the guard
  /// unsubscribes.
  pub fn subscribe_resource(&self, uri: &str) -> Result<ResourceGuard, String> {
    let tooling = self.inner.name();
    let uuid = self.inner.subscribe_resource(uri)?;
    Ok(ResourceGuard { tooling, uuid })
  }
}

/// A queued tool call; cancels delivery on drop.
pub struct CallGuard {
  tooling: String,
  uuid: String,
}

impl CallGuard {
  /// The UUID handle tagging this call's `tool-result` event.
  pub fn uuid(&self) -> &str {
    &self.uuid
  }

  /// Whether the call is still open.
  pub fn is_open(&self) -> bool {
    raw::get(&self.tooling).is_ok_and(|t| t.is_open(&self.uuid))
  }

  /// Cancel the call, dropping its pending delivery.
  pub fn cancel(&self) {
    if let Ok(t) = raw::get(&self.tooling) {
      t.cancel(&self.uuid);
    }
  }
}

impl Drop for CallGuard {
  fn drop(&mut self) {
    self.cancel();
  }
}

/// A resource-list subscription; unsubscribes on drop.
pub struct ResourceListGuard {
  tooling: String,
  uuid: String,
}

impl ResourceListGuard {
  /// The UUID handle tagging `resource-list-updated` events.
  pub fn uuid(&self) -> &str {
    &self.uuid
  }

  /// Cancel the subscription.
  pub fn unsubscribe(&self) {
    if let Ok(t) = raw::get(&self.tooling) {
      t.unsubscribe_resource_list(&self.uuid);
    }
  }
}

impl Drop for ResourceListGuard {
  fn drop(&mut self) {
    self.unsubscribe();
  }
}

/// A single-resource subscription; unsubscribes on drop.
pub struct ResourceGuard {
  tooling: String,
  uuid: String,
}

impl ResourceGuard {
  /// The UUID handle tagging `resource-updated` events.
  pub fn uuid(&self) -> &str {
    &self.uuid
  }

  /// Cancel the subscription.
  pub fn unsubscribe(&self) {
    if let Ok(t) = raw::get(&self.tooling) {
      t.unsubscribe_resource(&self.uuid);
    }
  }
}

impl Drop for ResourceGuard {
  fn drop(&mut self) {
    self.unsubscribe();
  }
}
