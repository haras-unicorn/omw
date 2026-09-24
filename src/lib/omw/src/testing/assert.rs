//! Assertion model, parser, and matcher for brain testing.
//!
//! `[assertions.<agent>]` in a test config is compared against the agent's
//! recorded trace: an optional terminal `outcome` plus an ordered `events`
//! list. Event assertions are matched as an **ordered subsequence** over
//! **partial patterns**: only the events actually written are checked, in
//! order, and unlisted events between them are skipped. A [`Pattern`] is a
//! partial JSON value whose string leaves are regular expressions and whose
//! other leaves are compared for equality.
//!
//! The same ordered-subsequence rules apply to arrays inside a pattern and to
//! the `events` list itself, with the shared `$while` / `$until` sentinels:
//! `{ "$while" = P }` greedily consumes a run of elements matching `P`, and
//! `{ "$until" = P }` skips ahead to the first element matching `P`, while
//! unlisted elements between matches are skipped and leading/trailing elements
//! are ignored.

use std::collections::BTreeMap;
use std::fmt::Write as _;
#[cfg(any(test, feature = "mock"))]
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, bail};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
#[cfg(any(test, feature = "mock"))]
use tokio::sync::{broadcast, watch};

use crate::host::events::Event;
#[cfg(any(test, feature = "mock"))]
use crate::host::trace::TraceSender;
use crate::host::trace::{AgentTrace, TraceEvent};
use crate::runtime::RunOutcome;

/// The flattened `[assertions]` section of a test config.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Assertions {
  #[serde(default)]
  pub assertions: BTreeMap<String, AgentAssertion>,
}

/// One agent's expected outcome and ordered event stream.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentAssertion {
  #[serde(default)]
  pub outcome: Option<OutcomeAssertion>,
  #[serde(default)]
  pub events: Vec<EventAssertion>,
}

/// The expected terminal outcome.
///
/// - `"completed"` — the iteration finished cleanly.
/// - `{ exited = "message" }` — the brain exited itself with that message.
/// - `"asserted"` — stop the agent once its `events` settle; the verdict is the
///   assertion result rather than an outcome. Only meaningful under the testing
///   harness; elsewhere the outcome is left unchecked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeAssertion {
  Named(String),
  Exited { exited: String },
  Asserted,
}

impl<'de> Deserialize<'de> for OutcomeAssertion {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: serde::Deserializer<'de>,
  {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
      Named(String),
      Exited { exited: String },
    }
    Ok(match Raw::deserialize(deserializer)? {
      Raw::Named(name) if name == "asserted" => Self::Asserted,
      Raw::Named(name) => Self::Named(name),
      Raw::Exited { exited } => Self::Exited { exited },
    })
  }
}

impl OutcomeAssertion {
  /// Whether `outcome` satisfies this assertion. `Asserted` never matches a
  /// concrete outcome: it is a stop directive the harness handles separately.
  pub fn matches(&self, outcome: &RunOutcome) -> bool {
    match (outcome, self) {
      (RunOutcome::Completed, Self::Named(name)) => name == "completed",
      (RunOutcome::Exited(message), Self::Exited { exited }) => {
        message == exited
      }
      _ => false,
    }
  }
}

/// Reserved pattern keys. `$`-prefixed keys inside an array element (or an
/// `events` entry) are the sequence sentinels, never partial-match fields.
const WHILE_KEY: &str = "$while";
const UNTIL_KEY: &str = "$until";

/// One expected observation, matched against the agent's ordered trace.
#[derive(Debug, Clone)]
pub enum EventAssertion {
  /// An outbound host call. `op` is exact; `detail` is a partial [`Pattern`]
  /// over the call's JSON detail.
  Call {
    op: Option<String>,
    detail: Option<Pattern>,
  },
  /// An inbound inbox event. `event` is the kebab-case kind (see
  /// [`event_kind`]); `payload` is a partial [`Pattern`] over the serialized
  /// [`Event`].
  Inbound {
    event: Option<String>,
    payload: Option<Pattern>,
  },
  /// Greedily consume a run of events matching the wrapped `call`/`inbound`
  /// assertion, stopping at the first non-match. Written as
  /// `{ "$while" = { kind = "call", ... } }`.
  While(Box<EventAssertion>),
  /// Skip events until one matches the wrapped assertion, consuming it.
  /// Written as `{ "$until" = { kind = "call", ... } }`.
  Until(Box<EventAssertion>),
}

impl<'de> Deserialize<'de> for EventAssertion {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: serde::Deserializer<'de>,
  {
    let value = Value::deserialize(deserializer)?;
    if let Value::Object(map) = &value {
      if let Some(inner) = map.get(WHILE_KEY) {
        return event_sentinel(inner, Self::While)
          .map_err(serde::de::Error::custom);
      }
      if let Some(inner) = map.get(UNTIL_KEY) {
        return event_sentinel(inner, Self::Until)
          .map_err(serde::de::Error::custom);
      }
    }
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "kebab-case")]
    enum Tagged {
      Call {
        #[serde(default)]
        op: Option<String>,
        #[serde(default)]
        detail: Option<Pattern>,
      },
      Inbound {
        #[serde(default)]
        event: Option<String>,
        #[serde(default)]
        payload: Option<Pattern>,
      },
    }
    match serde_json::from_value::<Tagged>(value)
      .map_err(serde::de::Error::custom)?
    {
      Tagged::Call { op, detail } => Ok(Self::Call { op, detail }),
      Tagged::Inbound { event, payload } => {
        Ok(Self::Inbound { event, payload })
      }
    }
  }
}

/// Wrap a `$while` / `$until` inner condition, rejecting anything that is not
/// a plain `call` / `inbound` assertion.
fn event_sentinel(
  inner: &Value,
  wrap: fn(Box<EventAssertion>) -> EventAssertion,
) -> anyhow::Result<EventAssertion> {
  let assertion = serde_json::from_value::<EventAssertion>(inner.clone())
    .context("invalid nested `$while` / `$until` assertion")?;
  match assertion {
    EventAssertion::Call { .. } | EventAssertion::Inbound { .. } => {
      Ok(wrap(Box::new(assertion)))
    }
    _ => bail!(
      "`{WHILE_KEY}` / `{UNTIL_KEY}` must wrap a `call` or `inbound` assertion"
    ),
  }
}

/// When a scripted step fires, shared by the mocks that accept an `after` gate.
///
/// - `"start"` fires as soon as the step is reached (the default).
/// - A `call` / `inbound` [`EventAssertion`] waits until a matching trace event
///   has been observed, scanning forward only.
#[derive(Debug, Clone, Default)]
pub enum After {
  /// `"start"`: fire as soon as the step is reached.
  #[default]
  Start,
  /// Wait until an event matching the pattern has been observed.
  Pattern(EventAssertion),
}

impl<'de> Deserialize<'de> for After {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: serde::Deserializer<'de>,
  {
    let value = Value::deserialize(deserializer)?;
    match value {
      Value::String(kind) if kind == "start" => Ok(Self::Start),
      Value::String(other) => Err(serde::de::Error::custom(format!(
        "`after` must be \"start\" or a pattern, got {other:?}"
      ))),
      other => EventAssertion::deserialize(other)
        .map(Self::Pattern)
        .map_err(serde::de::Error::custom),
    }
  }
}

impl After {
  /// Whether this gate fires immediately (no trace event to wait for).
  pub fn is_start(&self) -> bool {
    matches!(self, Self::Start)
  }
}

/// An append-only log of every trace event, with per-gate independent scanning.
///
/// A [`broadcast`] receiver only sees events sent after it subscribes, and a
/// single shared receiver forces consumers to steal each other's events. The
/// log fixes both: one drain task appends every event and each gate scans the
/// whole log independently, so any number of concurrent gates — across any
/// number of agents, which all share one tooling instance — can wait for events
/// that may already have happened.
#[cfg(any(test, feature = "mock"))]
#[derive(Debug)]
pub(crate) struct TraceLog {
  events: Mutex<Vec<TraceEvent>>,
  generation: watch::Sender<u64>,
}

#[cfg(any(test, feature = "mock"))]
impl TraceLog {
  /// Start logging `sender`'s events on a background task. The task ends when
  /// the sender (the run) drops.
  pub(crate) fn attach(sender: TraceSender) -> Arc<Self> {
    let (generation, _) = watch::channel(0);
    let log = Arc::new(Self {
      events: Mutex::new(Vec::new()),
      generation,
    });
    let logger = Arc::clone(&log);
    let mut receiver = sender.subscribe();
    tokio::spawn(async move {
      loop {
        match receiver.recv().await {
          Ok(event) => logger.push(event),
          Err(broadcast::error::RecvError::Lagged(_)) => continue,
          Err(broadcast::error::RecvError::Closed) => break,
        }
      }
    });
    log
  }

  /// Append one event and wake every waiting gate.
  fn push(&self, event: TraceEvent) {
    if let Ok(mut events) = self.events.lock() {
      events.push(event);
    }
    self
      .generation
      .send_modify(|generation| *generation = generation.saturating_add(1));
  }

  /// Wait until a trace event matching `after` has been observed.
  ///
  /// `"start"` (and a missing gate) returns immediately. A `$while` / `$until`
  /// gate resolves against its inner `call` / `inbound` condition. Because the
  /// log is append-only, gates never consume each other's events and may match
  /// an event that was observed before the gate was created.
  pub(crate) async fn wait_for(&self, after: &After) {
    let After::Pattern(condition) = after else {
      return;
    };
    let mut generation = self.generation.subscribe();
    loop {
      if self.is_observed(condition) {
        return;
      }
      if generation.changed().await.is_err() {
        return;
      }
    }
  }

  /// Whether a matching event is already in the log.
  fn is_observed(&self, condition: &EventAssertion) -> bool {
    self
      .events
      .lock()
      .map(|events| events.iter().any(|event| condition.matches(event)))
      .unwrap_or(false)
  }
}

/// A partial JSON pattern.
///
/// An object matches if every key it names is present in the candidate and
/// matches (extra candidate keys are ignored). A string leaf is a **regular
/// expression** matched against the candidate string. Numbers, booleans and
/// null are compared for equality. An array is an ordered subsequence of
/// [`ArrayStep`]s: unlisted elements between matches are skipped and leading
/// and trailing elements are ignored. An empty pattern array matches any array.
#[derive(Debug, Clone)]
pub enum Pattern {
  Regex(Regex),
  Exact(Value),
  Object(BTreeMap<String, Pattern>),
  Array(Vec<ArrayStep>),
}

/// One step of an array [`Pattern`].
#[derive(Debug, Clone)]
pub enum ArrayStep {
  /// Scan forward to the first element matching the inner pattern.
  Match(Pattern),
  /// `{ "$while" = P }`: greedily consume a run of elements matching `P`.
  While(Pattern),
  /// `{ "$until" = P }`: scan forward to the first element matching `P`.
  Until(Pattern),
}

impl Pattern {
  /// Whether `candidate` matches this pattern.
  pub fn matches(&self, candidate: &Value) -> bool {
    match self {
      Self::Regex(regex) => {
        candidate.as_str().is_some_and(|text| regex.is_match(text))
      }
      Self::Exact(value) => value == candidate,
      Self::Object(fields) => candidate.as_object().is_some_and(|object| {
        fields.iter().all(|(key, pattern)| {
          object.get(key).is_some_and(|value| pattern.matches(value))
        })
      }),
      Self::Array(steps) => candidate.as_array().is_some_and(|array| {
        let mut sequence = Sequence::default();
        for element in array {
          sequence.observe(steps, element);
        }
        sequence.is_done(steps)
      }),
    }
  }

  fn from_value(value: Value) -> anyhow::Result<Self> {
    Ok(match value {
      Value::String(source) => {
        Self::Regex(Regex::new(&source).with_context(|| {
          format!("invalid regex in assertion pattern: {source:?}")
        })?)
      }
      Value::Object(map) => Self::Object(
        map
          .into_iter()
          .map(|(key, value)| Ok((key, Self::from_value(value)?)))
          .collect::<anyhow::Result<_>>()?,
      ),
      Value::Array(items) => Self::Array(
        items
          .into_iter()
          .map(array_step_from_value)
          .collect::<anyhow::Result<_>>()?,
      ),
      other => Self::Exact(other),
    })
  }
}

/// Build one [`ArrayStep`], honoring the `$while` / `$until` sentinels.
fn array_step_from_value(value: Value) -> anyhow::Result<ArrayStep> {
  if let Value::Object(map) = &value {
    if let Some(inner) = map.get(WHILE_KEY) {
      return Ok(ArrayStep::While(Pattern::from_value(inner.clone())?));
    }
    if let Some(inner) = map.get(UNTIL_KEY) {
      return Ok(ArrayStep::Until(Pattern::from_value(inner.clone())?));
    }
  }
  Ok(ArrayStep::Match(Pattern::from_value(value)?))
}

impl<'de> Deserialize<'de> for Pattern {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: serde::Deserializer<'de>,
  {
    let value = Value::deserialize(deserializer)?;
    Self::from_value(value).map_err(serde::de::Error::custom)
  }
}

impl EventAssertion {
  /// Whether this assertion matches a single trace event. `While`/`Until`
  /// delegate to their inner `call`/`inbound` condition, so they also resolve
  /// against a single event for `after` gates.
  pub fn matches(&self, event: &TraceEvent) -> bool {
    match (self, event) {
      (
        Self::Call { op, detail },
        TraceEvent::Call {
          op: actual_op,
          detail: actual_detail,
          ..
        },
      ) => {
        op.as_ref().is_none_or(|op| op == actual_op)
          && detail
            .as_ref()
            .is_none_or(|pattern| pattern.matches(actual_detail))
      }
      (
        Self::Inbound {
          event: kind,
          payload,
        },
        TraceEvent::Inbound { event, .. },
      ) => {
        kind.as_ref().is_none_or(|kind| *kind == event_kind(event))
          && payload
            .as_ref()
            .is_none_or(|pattern| pattern.matches(&to_payload(event)))
      }
      (Self::While(inner) | Self::Until(inner), event) => inner.matches(event),
      _ => false,
    }
  }
}

/// The stable string name of an [`Event`] variant, as used in assertions.
pub fn event_kind(event: &Event) -> String {
  let kind = match event {
    Event::Message(_) => "message",
    Event::Error(_) => "error",
    Event::Timer => "timer",
    Event::Reload => "reload",
    Event::Shutdown => "shutdown",
    Event::ChatDelta(_) => "chat-delta",
    Event::ChatEnd => "chat-end",
    Event::ToolResult(_) => "tool-result",
    Event::ResourceListUpdated(_) => "resource-list-updated",
    Event::ResourceUpdated(_) => "resource-updated",
    Event::EndpointMessage(_) => "endpoint-message",
    Event::EndpointSessionEnd(_) => "endpoint-session-end",
  };
  kind.to_string()
}

/// The JSON projection of an inbound event, used for `payload` patterns.
fn to_payload(event: &Event) -> Value {
  serde_json::to_value(event).unwrap_or(Value::Null)
}

/// Group a flat trace stream into the per-agent view the matcher consumes.
pub fn collect(events: Vec<TraceEvent>) -> BTreeMap<String, AgentTrace> {
  crate::host::trace::group(events)
}

/// Compare the recorded run against the expected assertions, erroring with a
/// human-readable diff on the first mismatch.
pub fn check(
  actual: &BTreeMap<String, AgentTrace>,
  expected: &Assertions,
) -> anyhow::Result<()> {
  for (agent, assertion) in &expected.assertions {
    let empty = AgentTrace::default();
    let trace = actual.get(agent).unwrap_or(&empty);
    if let Some(expected_outcome) = &assertion.outcome
      && *expected_outcome != OutcomeAssertion::Asserted
    {
      let matched = trace
        .outcome
        .as_ref()
        .is_some_and(|outcome| expected_outcome.matches(outcome));
      if !matched {
        bail!(
          "agent {agent:?}: outcome mismatch\n  expected: {expected_outcome:?}\n  actual:   {:?}",
          trace.outcome
        );
      }
    }
    let mut matcher = Matcher::new(&assertion.events);
    for event in &trace.events {
      matcher.observe(event);
    }
    if !matcher.matched() {
      bail!("agent {agent:?}: {}", matcher.failure());
    }
  }
  Ok(())
}

/// How a sequence step advances the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepKind {
  /// Scan forward to the first matching element.
  Match,
  /// Greedily consume a run of matching elements.
  While,
  /// Scan forward to the first matching element (explicit form of `Match`).
  Until,
}

/// A step the shared incremental [`Sequence`] cursor knows how to advance over.
trait SequenceStep<Item> {
  /// This step's advancement rule.
  fn kind(&self) -> StepKind;
  /// Whether a `Match` / `While` / `Until` step matches `item`.
  fn matches(&self, item: &Item) -> bool;
}

/// The single incremental ordered-subsequence cursor shared by the events
/// [`Matcher`] and array [`Pattern`] matching, so the two can never drift.
///
/// Feed elements one at a time with [`observe`](Self::observe); a `Match` /
/// `Until` step scans forward, while a `While` step greedily consumes a run
/// and stops at the first non-match. Call [`is_done`](Self::is_done) afterwards
/// to learn whether every step was satisfied.
#[derive(Debug, Clone, Default)]
struct Sequence {
  /// Index of the next step to satisfy.
  next: usize,
}

impl Sequence {
  /// Whether every step has been satisfied. Trailing elements are ignored, and
  /// a pending `While` step is satisfied by zero elements (`$while` is
  /// zero-or-more).
  fn is_done<Item, S: SequenceStep<Item>>(&self, steps: &[S]) -> bool {
    let mut next = self.next;
    while steps
      .get(next)
      .is_some_and(|step| step.kind() == StepKind::While)
    {
      next = next.saturating_add(1);
    }
    next >= steps.len()
  }

  /// Consume one element, advancing the cursor if it satisfies the next
  /// pending step.
  fn observe<Item, S: SequenceStep<Item>>(&mut self, steps: &[S], item: &Item) {
    loop {
      if self.is_done(steps) {
        return;
      }
      let Some(step) = steps.get(self.next) else {
        return;
      };
      match step.kind() {
        StepKind::Match | StepKind::Until => {
          if step.matches(item) {
            self.next = self.next.saturating_add(1);
          }
          return;
        }
        StepKind::While => {
          if step.matches(item) {
            return;
          }
          self.next = self.next.saturating_add(1);
        }
      }
    }
  }
}

impl SequenceStep<TraceEvent> for EventAssertion {
  fn kind(&self) -> StepKind {
    match self {
      Self::Call { .. } | Self::Inbound { .. } => StepKind::Match,
      Self::While(_) => StepKind::While,
      Self::Until(_) => StepKind::Until,
    }
  }

  fn matches(&self, event: &TraceEvent) -> bool {
    Self::matches(self, event)
  }
}

impl SequenceStep<Value> for ArrayStep {
  fn kind(&self) -> StepKind {
    match self {
      Self::Match(_) => StepKind::Match,
      Self::While(_) => StepKind::While,
      Self::Until(_) => StepKind::Until,
    }
  }

  fn matches(&self, item: &Value) -> bool {
    match self {
      Self::Match(pattern) | Self::While(pattern) | Self::Until(pattern) => {
        pattern.matches(item)
      }
    }
  }
}

/// An incremental ordered-subsequence matcher over one agent's event
/// assertions. Feed it trace events in order with [`observe`](Self::observe);
/// [`matched`](Self::matched) reports whether every assertion is satisfied.
/// The testing harness drives this live; [`check`] drives it over a recorded
/// trace.
#[derive(Debug, Clone)]
pub struct Matcher {
  assertions: Vec<EventAssertion>,
  sequence: Sequence,
  /// Every observed event, kept for diff rendering.
  events: Vec<TraceEvent>,
}

impl Matcher {
  /// A matcher for `assertions`.
  pub fn new(assertions: &[EventAssertion]) -> Self {
    Self {
      assertions: assertions.to_vec(),
      sequence: Sequence::default(),
      events: Vec::new(),
    }
  }

  /// Whether every assertion has been satisfied. Trailing events are ignored.
  pub fn matched(&self) -> bool {
    self.sequence.is_done(&self.assertions)
  }

  /// Consume one trace event, advancing the matcher if it satisfies the next
  /// pending assertion.
  pub fn observe(&mut self, event: &TraceEvent) {
    self.events.push(event.clone());
    self.sequence.observe(&self.assertions, event);
  }

  /// A readable diff of what is still unsatisfied, for a failed assertion.
  pub fn failure(&self) -> String {
    let next = self.sequence.next;
    let mut out = String::new();
    let _ =
      writeln!(out, "assertion #{} not satisfied", next.saturating_add(1));
    let _ = writeln!(
      out,
      "  expected: {}",
      render_remaining(&self.assertions[next.min(self.assertions.len())..])
    );
    let _ = write!(out, "  observed: {}", render_events(&self.events));
    out
  }
}

fn render_events(events: &[TraceEvent]) -> String {
  let parts: Vec<String> = events.iter().map(render_event).collect();
  format!("[{}]", parts.join(", "))
}

fn render_remaining(assertions: &[EventAssertion]) -> String {
  let parts: Vec<String> = assertions.iter().map(render_assertion).collect();
  format!("[{}]", parts.join(", "))
}

fn render_event(event: &TraceEvent) -> String {
  match event {
    TraceEvent::Call { op, detail, .. } => {
      format!("call(op={op:?}, detail={detail})")
    }
    TraceEvent::Inbound { event, .. } => {
      format!("inbound({})", event_kind(event))
    }
    TraceEvent::Outcome { outcome, .. } => format!("outcome({outcome:?})"),
  }
}

fn render_assertion(assertion: &EventAssertion) -> String {
  match assertion {
    EventAssertion::Call { op, detail } => format!(
      "call(op={op:?}, detail={})",
      render_optional_pattern(detail)
    ),
    EventAssertion::Inbound { event, payload } => format!(
      "inbound(event={event:?}, payload={})",
      render_optional_pattern(payload)
    ),
    EventAssertion::While(inner) => {
      format!("{{{WHILE_KEY} = {}}}", render_assertion(inner))
    }
    EventAssertion::Until(inner) => {
      format!("{{{UNTIL_KEY} = {}}}", render_assertion(inner))
    }
  }
}

fn render_optional_pattern(pattern: &Option<Pattern>) -> String {
  match pattern {
    Some(pattern) => render_pattern(pattern),
    None => "none".to_string(),
  }
}

fn render_pattern(pattern: &Pattern) -> String {
  match pattern {
    Pattern::Regex(regex) => format!("/{}/", regex.as_str()),
    Pattern::Exact(value) => value.to_string(),
    Pattern::Object(fields) => {
      let parts: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{key}: {}", render_pattern(value)))
        .collect();
      format!("{{{}}}", parts.join(", "))
    }
    Pattern::Array(steps) => {
      let parts: Vec<String> = steps.iter().map(render_array_step).collect();
      format!("[{}]", parts.join(", "))
    }
  }
}

fn render_array_step(step: &ArrayStep) -> String {
  match step {
    ArrayStep::Match(pattern) => render_pattern(pattern),
    ArrayStep::While(pattern) => {
      format!("{{{WHILE_KEY} = {}}}", render_pattern(pattern))
    }
    ArrayStep::Until(pattern) => {
      format!("{{{UNTIL_KEY} = {}}}", render_pattern(pattern))
    }
  }
}

/// Parse the `[assertions]` section out of a test config.
pub fn parse(source: &str) -> anyhow::Result<Assertions> {
  toml::from_str(source).context("failed to deserialize [assertions]")
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::host::events::ToolResult;
  use crate::provider::ChatDelta;
  use serde_json::json;
  use std::time::Duration;

  fn call(op: &str, detail: Value) -> TraceEvent {
    TraceEvent::Call {
      agent: "alice".to_string(),
      op: op.to_string(),
      detail,
    }
  }

  fn after_call(op: &str) -> After {
    After::Pattern(EventAssertion::Call {
      op: Some(op.to_string()),
      detail: None,
    })
  }

  #[tokio::test]
  async fn trace_log_sees_pre_existing_and_future_events() {
    let (tx, _rx) = broadcast::channel(16);
    let log = TraceLog::attach(tx.clone());
    tx.send(call("first", json!({}))).unwrap();

    // A gate may match an event observed before it was created...
    tokio::time::timeout(
      Duration::from_secs(5),
      log.wait_for(&after_call("first")),
    )
    .await
    .expect("a pre-existing event should satisfy the gate");

    // ...and still wait for a future one.
    let emitter = tx.clone();
    tokio::spawn(async move {
      tokio::time::sleep(Duration::from_millis(20)).await;
      let _ = emitter.send(call("later", json!({})));
    });
    tokio::time::timeout(
      Duration::from_secs(5),
      log.wait_for(&after_call("later")),
    )
    .await
    .expect("a future event should satisfy the gate");
  }

  #[tokio::test]
  async fn trace_log_gates_do_not_consume_each_other() {
    let (tx, _rx) = broadcast::channel(16);
    let log = TraceLog::attach(tx.clone());
    tx.send(call("first", json!({}))).unwrap();
    tx.send(call("second", json!({}))).unwrap();

    // Two concurrent gates on distinct events both settle; neither steals the
    // other's event.
    let first = after_call("first");
    let second = after_call("second");
    tokio::time::timeout(Duration::from_secs(5), async {
      tokio::join!(log.wait_for(&first), log.wait_for(&second));
    })
    .await
    .expect("both gates should settle");
  }

  fn inbound(event: Event) -> TraceEvent {
    TraceEvent::Inbound {
      agent: "alice".to_string(),
      id: "sub".to_string(),
      event,
    }
  }

  fn delta(content: &str) -> Event {
    Event::ChatDelta(ChatDelta {
      content: Some(content.to_string()),
      tool_call: None,
      finish_reason: None,
    })
  }

  fn outcome(completed: bool) -> TraceEvent {
    TraceEvent::Outcome {
      agent: "alice".to_string(),
      outcome: if completed {
        RunOutcome::Completed
      } else {
        RunOutcome::Exited("bye".to_string())
      },
    }
  }

  fn assertions(agent: &str, body: &str) -> Assertions {
    let rendered = format!("[assertions.{agent}]\n{body}");
    parse(&rendered).expect("assertions should parse")
  }

  fn assert_check(agent: &str, body: &str, events: Vec<TraceEvent>) {
    let expected = assertions(agent, body);
    check(&collect(events), &expected).expect("check should pass");
  }

  fn assert_failure(
    agent: &str,
    body: &str,
    events: Vec<TraceEvent>,
  ) -> String {
    let expected = assertions(agent, body);
    check(&collect(events), &expected)
      .expect_err("check should fail")
      .to_string()
  }

  #[test]
  fn pattern_objects_match_partially() {
    let pattern =
      Pattern::from_value(json!({ "model": "gpt-4o" })).expect("pattern");
    assert!(pattern.matches(&json!({ "model": "gpt-4o", "provider": "m" })));
    assert!(!pattern.matches(&json!({ "provider": "m" })));
  }

  #[test]
  fn pattern_strings_are_regexes() {
    let pattern =
      Pattern::from_value(json!({ "tool": "^get_.*" })).expect("pattern");
    assert!(pattern.matches(&json!({ "tool": "get_weather" })));
    assert!(!pattern.matches(&json!({ "tool": "set_weather" })));
  }

  #[test]
  fn pattern_arrays_match_as_ordered_subsequences() {
    let pattern =
      Pattern::from_value(json!({ "ids": ["a", "b"] })).expect("pattern");
    // Leading, between and trailing elements are ignored.
    assert!(pattern.matches(&json!({ "ids": ["a", "b"] })));
    assert!(pattern.matches(&json!({ "ids": ["x", "a", "b", "y"] })));
    assert!(pattern.matches(&json!({ "ids": ["a", "x", "b"] })));
    // Out of order or incomplete is a mismatch.
    assert!(!pattern.matches(&json!({ "ids": ["b", "a"] })));
    assert!(!pattern.matches(&json!({ "ids": ["a"] })));
    assert!(!pattern.matches(&json!({ "ids": [] })));
  }

  #[test]
  fn pattern_empty_array_matches_any_array() {
    let pattern = Pattern::from_value(json!({ "ids": [] })).expect("pattern");
    assert!(pattern.matches(&json!({ "ids": [] })));
    assert!(pattern.matches(&json!({ "ids": ["anything"] })));
    assert!(!pattern.matches(&json!({ "ids": "not an array" })));
  }

  #[test]
  fn while_consumes_runs_and_until_scans_ahead() {
    // `$while` consumes a greedy run, `$until` skips to the first match.
    let pattern = Pattern::from_value(json!({
      "ids": [{ "$while": "a" }, { "$until": "d" }, { "$while": "e" }]
    }))
    .expect("pattern");
    assert!(pattern.matches(&json!({
      "ids": ["a", "a", "b", "d", "e", "e", "f"]
    })));

    // `$while` is zero-or-more: an immediately non-matching element is fine,
    // and a trailing run is satisfied by the remaining elements.
    let zero = Pattern::from_value(json!({ "ids": [{ "$while": "a" }] }))
      .expect("pattern");
    assert!(zero.matches(&json!({ "ids": [] })));
    assert!(zero.matches(&json!({ "ids": ["b"] })));
    assert!(zero.matches(&json!({ "ids": ["a", "a"] })));

    // The same sentinel shapes drive the events matcher.
    assert_check(
      "alice",
      r#"events = [{ "$while" = { kind = "call", op = "a" } }, { kind = "call", op = "d" }]"#,
      vec![
        call("a", json!({})),
        call("a", json!({})),
        call("d", json!({})),
      ],
    );
    assert_check(
      "alice",
      r#"events = [{ "$until" = { kind = "call", op = "d" } }]"#,
      vec![call("a", json!({})), call("d", json!({}))],
    );
  }

  #[test]
  fn empty_object_under_while_or_until_matches_any_object() {
    let until = Pattern::from_value(json!({ "ids": [{ "$until": {} }] }))
      .expect("pattern");
    assert!(until.matches(&json!({ "ids": [{ "n": 1 }, { "n": 2 }] })));
    // Objects only: a string element cannot satisfy the wildcard.
    assert!(!until.matches(&json!({ "ids": ["x"] })));

    let while_run = Pattern::from_value(json!({ "ids": [{ "$while": {} }] }))
      .expect("pattern");
    assert!(while_run.matches(&json!({ "ids": [] })));
    assert!(while_run.matches(&json!({ "ids": [{ "n": 1 }, { "n": 2 }] })));
  }

  #[test]
  fn pattern_array_step_render_uses_the_sentinels() {
    let pattern = Pattern::from_value(json!({
      "ids": [{ "$while": "a" }, { "$until": "z" }]
    }))
    .expect("pattern");
    assert_eq!(
      render_pattern(&pattern),
      "{ids: [{$while = /a/}, {$until = /z/}]}"
    );
  }

  #[test]
  fn nested_while_and_until_are_rejected() {
    parse(
      "[assertions.alice]\nevents = [{ \"$while\" = { \"$until\" = { kind = \"call\" } } }]\n",
    )
    .expect_err("`$while` may not wrap another sentinel");
    parse(
      "[assertions.alice]\nevents = [{ \"$until\" = { \"$while\" = { kind = \"call\" } } }]\n",
    )
    .expect_err("`$until` may not wrap another sentinel");
  }

  #[test]
  fn pattern_nested_objects() {
    let pattern = Pattern::from_value(json!({
      "outer": { "inner": { "leaf": "x.*" } }
    }))
    .expect("pattern");
    assert!(pattern.matches(&json!({
      "outer": { "inner": { "leaf": "xyz" }, "extra": 1 }
    })));
    assert!(!pattern.matches(&json!({ "outer": { "inner": {} } })));
  }

  #[test]
  fn invalid_regex_is_rejected_at_parse() {
    let error = parse(
      "[assertions.alice]\nevents = [{ kind = \"call\", detail = { x = \"[\" } }]\n",
    )
    .expect_err("invalid regex should fail to parse");
    assert!(format!("{error:#}").contains("invalid regex"), "{error:#}");
  }

  #[test]
  fn outcome_asserted_parses_and_needs_no_outcome() {
    let expected = assertions("alice", "outcome = \"asserted\"\nevents = []");
    assert_eq!(
      expected.assertions["alice"].outcome,
      Some(OutcomeAssertion::Asserted)
    );
    check(&collect(vec![call("chat", json!({}))]), &expected)
      .expect("asserted outcome is unchecked");
  }

  #[test]
  fn check_passes_on_an_exact_match() -> anyhow::Result<()> {
    let events = vec![
      call("chat", json!({ "model": "gpt-4o" })),
      inbound(delta("hi")),
      outcome(true),
    ];
    assert_check(
      "alice",
      r#"
        outcome = "completed"
        events = [
          { kind = "call", op = "chat" },
          { kind = "inbound", event = "chat-delta" },
        ]
      "#,
      events,
    );
    Ok(())
  }

  #[test]
  fn subsequence_skips_unlisted_events() {
    let events = vec![
      call("list_tools", json!({})),
      inbound(delta("thinking")),
      call("chat", json!({ "model": "gpt-4o" })),
      inbound(Event::ChatEnd),
      outcome(true),
    ];
    assert_check(
      "alice",
      r#"
        events = [
          { kind = "call", op = "chat", detail = { model = "gpt-4o" } },
        ]
      "#,
      events,
    );
  }

  #[test]
  fn while_consumes_a_run_of_events() {
    let events = vec![
      call("delta", json!({})),
      call("delta", json!({})),
      call("final", json!({})),
    ];
    assert_check(
      "alice",
      r#"events = [{ "$while" = { kind = "call", op = "delta" } }, { kind = "call", op = "final" }]"#,
      events,
    );
    let error = assert_failure(
      "alice",
      r#"events = [{ "$while" = { kind = "call", op = "delta" } }, { kind = "call", op = "final" }]"#,
      vec![call("delta", json!({})), call("other", json!({}))],
    );
    assert!(error.contains("not satisfied"));
  }

  #[test]
  fn until_consumes_the_first_matching_event() {
    let events = vec![
      call("a", json!({})),
      call("b", json!({})),
      call("c", json!({})),
    ];
    assert_check(
      "alice",
      r#"events = [{ "$until" = { kind = "call", op = "c" } }]"#,
      events,
    );
  }

  #[test]
  fn a_trailing_while_is_satisfied_before_any_event() {
    let expected = assertions(
      "alice",
      r#"events = [{ "$while" = { kind = "call", op = "delta" } }]"#,
    );
    let matcher = Matcher::new(&expected.assertions["alice"].events);
    assert!(matcher.matched(), "a trailing `$while` is zero-or-more");
  }

  #[test]
  fn legacy_any_and_skip_tags_are_rejected() {
    parse("[assertions.alice]\nevents = [{ kind = \"any\" }]\n")
      .expect_err("`kind = \"any\"` should no longer parse");
    parse("[assertions.alice]\nevents = [{ kind = \"skip\", count = 1 }]\n")
      .expect_err("`kind = \"skip\"` should no longer parse");
  }

  #[test]
  fn prefix_match_ignores_trailing_events() {
    let events = vec![call("a", json!({})), call("b", json!({}))];
    assert_check("alice", r#"events = [{ kind = "call", op = "a" }]"#, events);
  }

  #[test]
  fn detail_pattern_disambiguates_lookalikes() {
    let events = vec![
      call("chat", json!({ "model": "gpt-3.5" })),
      call("chat", json!({ "model": "gpt-4o" })),
    ];
    assert_check(
      "alice",
      r#"events = [{ kind = "call", op = "chat", detail = { model = "gpt-4o" } }]"#,
      events,
    );
  }

  #[test]
  fn chat_detail_messages_match_as_a_subsequence() {
    let events = vec![call(
      "chat",
      json!({
        "provider": "mock",
        "model": "gpt-4o",
        "messages": [
          { "role": "system", "content": "be brief" },
          { "role": "user", "content": "a" },
          { "role": "assistant", "content": "b" },
          { "role": "user", "content": "final" },
        ],
        "tools": [{ "name": "echo" }],
      }),
    )];
    assert_check(
      "alice",
      r#"
        events = [
          {
            kind = "call",
            op = "chat",
            detail = {
              messages = [
                { role = "system" },
                { "$while" = { role = "assistant" } },
                { role = "user", content = "final" },
              ],
              tools = [ { name = "echo" } ],
            },
          },
        ]
      "#,
      events,
    );
  }

  #[test]
  fn inbound_payload_pattern_matches() {
    let events = vec![inbound(delta("hello, world"))];
    assert_check(
      "alice",
      r#"
        events = [
          {
            kind = "inbound",
            event = "chat-delta",
            payload = { kind = "chat-delta", data = { content = "hello, w.*" } },
          },
        ]
      "#,
      events,
    );
  }

  #[test]
  fn inbound_payload_pattern_can_match_a_tool_result() {
    let events = vec![inbound(Event::ToolResult(ToolResult {
      name: "get_weather".to_string(),
      arguments: "{}".to_string(),
      result: "sunny".to_string(),
    }))];
    assert_check(
      "alice",
      r#"
        events = [
          {
            kind = "inbound",
            event = "tool-result",
            payload = { data = { name = "get_weather", result = "sunny" } },
          },
        ]
      "#,
      events,
    );
  }

  #[test]
  fn call_without_op_matches_any_call() {
    let events = vec![call("subscribe_endpoint", json!({}))];
    assert_check("alice", r#"events = [{ kind = "call" }]"#, events);
  }

  #[test]
  fn mismatch_reports_a_readable_diff() {
    let events = vec![inbound(Event::ChatEnd)];
    let error = assert_failure(
      "alice",
      r#"events = [{ kind = "call", op = "chat" }]"#,
      events,
    );
    assert!(error.contains("assertion #1 not satisfied"), "{error}");
    assert!(error.contains("call(op=Some(\"chat\")"), "{error}");
    assert!(error.contains("inbound(chat-end)"), "{error}");
  }

  #[test]
  fn check_reports_an_event_order_mismatch() {
    let events = vec![inbound(Event::ChatEnd), call("chat", json!({}))];
    let expected = assertions(
      "alice",
      r#"
        events = [
          { kind = "call", op = "chat" },
          { kind = "inbound", event = "chat-end" },
        ]
      "#,
    );
    let error =
      check(&collect(events), &expected).expect_err("should mismatch");
    assert!(error.to_string().contains("not satisfied"));
  }

  #[test]
  fn check_reports_an_outcome_mismatch() {
    let events = vec![outcome(false)];
    let expected = assertions("alice", r#"outcome = "completed""#);
    let error =
      check(&collect(events), &expected).expect_err("should mismatch");
    assert!(error.to_string().contains("outcome mismatch"));
  }

  #[test]
  fn check_matches_an_exited_outcome() {
    assert_check(
      "alice",
      r#"outcome = { exited = "bye" }"#,
      vec![outcome(false)],
    );
  }

  #[test]
  fn check_ignores_agents_not_listed() {
    // `bob` has no trace: an empty expected list matches an empty actual one.
    assert_check("bob", r#"events = []"#, vec![call("chat", json!({}))]);
  }

  #[test]
  fn assertions_default_to_empty() -> anyhow::Result<()> {
    let parsed = parse("[runtime.rhai]\nkind = \"rhai\"\n")?;
    assert!(parsed.assertions.is_empty());
    Ok(())
  }
}
