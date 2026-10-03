//! The `tracing` layer that feeds the live view.
//!
//! Each event is formatted into a single line and routed by its `source` /
//! `tooling` fields: MCP child stderr lands in an `mcp:<name>` tab, everything
//! else in the default `omw` tab. In the [`View::Tests`] view the line is
//! buffered under the test currently running so a failure can replay it.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::live::{State, View, Wake};

/// A `tracing` layer that pushes formatted events into the live view.
#[derive(Debug)]
pub(crate) struct TuiLayer {
  state: Arc<Mutex<State>>,
  wake: Wake,
}

impl TuiLayer {
  pub(crate) fn new(state: Arc<Mutex<State>>, wake: Wake) -> Self {
    Self { state, wake }
  }
}

impl<S: Subscriber> Layer<S> for TuiLayer {
  fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
    let mut fields = Fields::default();
    event.record(&mut fields);
    let metadata = event.metadata();
    let line = fields.render(metadata.level(), metadata.target());

    let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
    match state.view {
      View::Agents => state.push_line(&fields.tab(), line),
      View::Tests => {
        if state.current_test.is_some() {
          state.current_logs.push(line);
        }
      }
    }
    drop(state);
    self.wake.send();
  }
}

/// The event fields the live view cares about, plus a rendered line.
#[derive(Debug, Default)]
struct Fields {
  message: String,
  source: Option<String>,
  tooling: Option<String>,
  extra: Vec<(String, String)>,
}

impl Fields {
  /// The tab this event belongs to.
  fn tab(&self) -> String {
    match (&self.source, &self.tooling) {
      (Some(source), Some(tooling)) if source == "mcp" => {
        format!("mcp:{tooling}")
      }
      _ => "omw".to_owned(),
    }
  }

  /// Render `level`, `target` and the fields into one line.
  fn render(&self, level: &Level, target: &str) -> String {
    let mut line = format!("{level:>5} {target}: {}", self.message);
    for (key, value) in &self.extra {
      line.push(' ');
      line.push_str(key);
      line.push('=');
      line.push_str(value);
    }
    line
  }
}

impl Visit for Fields {
  fn record_str(&mut self, field: &Field, value: &str) {
    match field.name() {
      "message" => self.message = value.to_owned(),
      "source" => self.source = Some(value.to_owned()),
      "tooling" => self.tooling = Some(value.to_owned()),
      other => self.extra.push((other.to_owned(), value.to_owned())),
    }
  }

  fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
    match field.name() {
      "message" => self.message = format!("{value:?}"),
      "source" => self.source = Some(format!("{value:?}")),
      "tooling" => self.tooling = Some(format!("{value:?}")),
      other => self.extra.push((other.to_owned(), format!("{value:?}"))),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn mcp_events_route_to_a_named_tab() {
    let fields = Fields {
      source: Some("mcp".to_owned()),
      tooling: Some("everything".to_owned()),
      ..Fields::default()
    };
    assert_eq!(fields.tab(), "mcp:everything");
  }

  #[test]
  fn other_events_route_to_the_default_tab() {
    let fields = Fields::default();
    assert_eq!(fields.tab(), "omw");

    let fields = Fields {
      source: Some("mcp".to_owned()),
      ..Fields::default()
    };
    assert_eq!(fields.tab(), "omw");
  }

  #[test]
  fn render_includes_level_target_message_and_extra_fields() {
    let fields = Fields {
      message: "hello".to_owned(),
      extra: vec![("count".to_owned(), "3".to_owned())],
      ..Fields::default()
    };
    assert_eq!(
      fields.render(&Level::INFO, "omw"),
      " INFO omw: hello count=3"
    );
  }
}
