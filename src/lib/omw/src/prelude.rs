//! Common imports for embedding `omw` as a library.
//!
//! ```rust,ignore
//! use omw::prelude::*;
//! ```
//!
//! The per-family `Factory` traits share one name, so they are re-exported
//! here under aliased names (`ProviderFactory`, `ToolingFactory`,
//! `RuntimeFactory`, `EndpointFactory`). The original paths
//! (`omw::provider::Factory`, …) keep working.

pub use crate::agent::{
  Registries, loop_agents, loop_agents_traced, run_agents, run_agents_traced,
};
pub use crate::config::{AgentConfig, Config, ImplConfig, Tunables};
pub use crate::endpoint::{
  Endpoint, EndpointEntry, Factory as EndpointFactory,
};
pub use crate::host::ctx::AgentContext;
pub use crate::host::events::{Event, EventEnvelope};
pub use crate::host::trace::{AgentTrace, TraceEvent, TraceSender};
pub use crate::provider::{
  ChatDelta, ChatMessage, ChatResult, Factory as ProviderFactory, Provider,
  ProviderEntry, Role, ToolCall,
};
pub use crate::runtime::{
  Factory as RuntimeFactory, RunOutcome, Runtime, RuntimeEntry,
};
pub use crate::secret::Secret;
pub use crate::shutdown::Shutdown;
pub use crate::testing::{
  AgentAssertion, AgentReport, ArrayStep, Assertions, EventAssertion, Harness,
  Matcher, OutcomeAssertion, Pattern, Report, check, collect, event_kind,
  parse,
};
pub use crate::tooling::{
  Factory as ToolingFactory, ResourceContent, ResourceInfo,
  ResourceNotification, Tool, Tooling, ToolingEntry,
};
pub use crate::watch::{RecursiveMode, Scripts, Watcher, scope};
pub use crate::{
  register_endpoints, register_providers, register_runtimes, register_toolings,
};
