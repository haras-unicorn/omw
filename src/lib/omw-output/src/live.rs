//! The live-view handle and the shared state the render thread draws.
//!
//! The tracing layer and the binaries both write into [`State`]; the render
//! thread reads it. The [`Live`] handle owns the render thread and restores the
//! terminal on drop.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

/// Which live view a binary drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
  /// `omw-cli`: one tab per log source plus a status line.
  #[default]
  Agents,
  /// `omw-test`: a progress gauge, per-test verdicts and failure detail.
  Tests,
}

/// The default render-loop tick, kept in sync with `omw`'s
/// `default_tui_tick_ms` (this crate cannot depend on `omw`).
const DEFAULT_TICK_MS: u64 = 80;

/// The default per-tab log cap, kept in sync with `omw`'s
/// `default_tui_tab_capacity`; `0` means unlimited.
const DEFAULT_TAB_CAPACITY: usize = 2000;

/// Where one test is in the [`View::Tests`] run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestStatus {
  /// Discovered but not started.
  Pending,
  /// Currently executing.
  Running,
  /// Finished and satisfied.
  Passed,
  /// Finished and unsatisfied.
  Failed,
}

/// One discovered test in the [`View::Tests`] list.
#[derive(Debug, Clone)]
pub(crate) struct TestEntry {
  /// The test's label.
  pub(crate) label: String,
  /// Where it is in the run.
  pub(crate) status: TestStatus,
}

/// A failure to render in the [`View::Tests`] view.
#[derive(Debug, Clone)]
pub(crate) struct Failure {
  /// The failing test's label.
  pub(crate) label: String,
  /// The rendered assertion diff (expected vs observed).
  pub(crate) detail: String,
  /// The test's buffered log lines, replayed with the failure.
  pub(crate) logs: Vec<String>,
}

/// One tab in the [`View::Agents`] view.
#[derive(Debug)]
pub(crate) struct Tab {
  /// The tab title: `omw` or `mcp:<name>`.
  pub(crate) name: String,
  /// The tab's buffered log lines.
  pub(crate) lines: VecDeque<String>,
  /// How many lines the user has scrolled up from the bottom.
  pub(crate) scroll: usize,
}

/// The shared live-view state.
#[derive(Debug)]
pub(crate) struct State {
  /// Which view is rendered.
  pub(crate) view: View,
  /// The launched command line, shown in the info panel.
  pub(crate) command: String,
  /// Extra `key · value` details for the info panel.
  pub(crate) details: Vec<(String, String)>,
  /// The tabs, `omw` first.
  pub(crate) tabs: Vec<Tab>,
  /// The selected tab.
  pub(crate) active: usize,
  /// The bottom status line.
  pub(crate) status: String,
  /// Whether the spinner animates.
  pub(crate) busy: bool,
  /// The spinner frame counter.
  pub(crate) spinner: usize,
  /// Whether color is applied (`NO_COLOR` unset).
  pub(crate) color: bool,
  /// How long the render loop waits between redraws.
  pub(crate) tick: Duration,
  /// The per-tab log cap, or `None` when unlimited.
  pub(crate) tab_capacity: Option<usize>,
  /// Tests finished so far.
  pub(crate) progress_done: usize,
  /// Tests discovered in this pass.
  pub(crate) progress_total: usize,
  /// Every discovered test, in run order, with its status.
  pub(crate) tests: Vec<TestEntry>,
  /// The label of the test currently running, if any.
  pub(crate) current_test: Option<String>,
  /// Log lines buffered for the test currently running.
  pub(crate) current_logs: Vec<String>,
  /// How many lines the user has scrolled up from the bottom (`View::Tests`).
  pub(crate) logs_scroll: usize,
  /// The most recent failure to render.
  pub(crate) failure: Option<Failure>,
}

impl State {
  pub(crate) fn new(color: bool) -> Self {
    Self {
      view: View::Agents,
      command: String::new(),
      details: Vec::new(),
      tabs: vec![Tab {
        name: "omw".to_owned(),
        lines: VecDeque::new(),
        scroll: 0,
      }],
      active: 0,
      status: String::new(),
      busy: false,
      spinner: 0,
      color,
      tick: Duration::from_millis(DEFAULT_TICK_MS),
      tab_capacity: (DEFAULT_TAB_CAPACITY != 0).then_some(DEFAULT_TAB_CAPACITY),
      progress_done: 0,
      progress_total: 0,
      tests: Vec::new(),
      current_test: None,
      current_logs: Vec::new(),
      logs_scroll: 0,
      failure: None,
    }
  }

  /// The index of the test labelled `label`, if present.
  pub(crate) fn test_index(&self, label: &str) -> Option<usize> {
    self.tests.iter().position(|test| test.label == label)
  }

  /// The index of the tab named `name`, creating it (empty) if needed.
  fn tab_index(&mut self, name: &str) -> usize {
    if let Some(index) = self.tabs.iter().position(|tab| tab.name == name) {
      return index;
    }
    self.tabs.push(Tab {
      name: name.to_owned(),
      lines: VecDeque::new(),
      scroll: 0,
    });
    self.tabs.len().saturating_sub(1)
  }

  /// Append `line` to the tab named `name`, dropping the oldest past the cap.
  pub(crate) fn push_line(&mut self, name: &str, line: String) {
    let index = self.tab_index(name);
    let capacity = self.tab_capacity;
    if let Some(tab) = self.tabs.get_mut(index) {
      if let Some(capacity) = capacity
        && tab.lines.len() >= capacity
      {
        tab.lines.pop_front();
      }
      tab.lines.push_back(line);
    }
  }
}

/// A wake signal shared by the tracing layer and the [`Live`] handle.
///
/// `std::sync::mpsc::Sender` is not `Sync`, so it is wrapped in a `Mutex` to
/// let both the layer (installed globally) and the handle use it.
#[derive(Clone, Debug)]
pub(crate) struct Wake {
  tx: Arc<Mutex<Sender<()>>>,
}

impl Wake {
  pub(crate) fn new(tx: Sender<()>) -> Self {
    Self {
      tx: Arc::new(Mutex::new(tx)),
    }
  }

  pub(crate) fn send(&self) {
    let guard = self.tx.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = guard.send(());
  }
}

/// A handle to the running live view.
///
/// Dropping it stops the render thread and restores the terminal.
#[derive(Debug)]
pub struct Live {
  state: Arc<Mutex<State>>,
  wake: Wake,
  quit: Mutex<Option<Receiver<()>>>,
  stop: Arc<AtomicBool>,
  handle: Mutex<Option<JoinHandle<()>>>,
}

impl Live {
  pub(crate) fn new(
    state: Arc<Mutex<State>>,
    wake: Wake,
    quit: Receiver<()>,
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
  ) -> Self {
    Self {
      state,
      wake,
      quit: Mutex::new(Some(quit)),
      stop,
      handle: Mutex::new(Some(handle)),
    }
  }

  fn lock(&self) -> MutexGuard<'_, State> {
    self.state.lock().unwrap_or_else(PoisonError::into_inner)
  }

  /// Select the rendered view.
  pub fn set_view(&self, view: View) {
    self.lock().view = view;
    self.wake.send();
  }

  /// Apply the tunables-driven knobs: the redraw interval and the per-tab log
  /// cap (`0` means unlimited). Best-effort; the crate-local defaults hold
  /// until the binary calls this after loading its config.
  pub fn configure(&self, tick_ms: u64, tab_capacity: usize) {
    let mut state = self.lock();
    state.tick = Duration::from_millis(tick_ms);
    state.tab_capacity = (tab_capacity != 0).then_some(tab_capacity);
    drop(state);
    self.wake.send();
  }

  /// Set the launched command line shown in the info panel.
  pub fn set_command(&self, command: impl Into<String>) {
    self.lock().command = command.into();
    self.wake.send();
  }

  /// Set the `key · value` details shown in the info panel.
  pub fn set_details(&self, details: Vec<(String, String)>) {
    self.lock().details = details;
    self.wake.send();
  }

  /// Set the bottom status line.
  pub fn set_status(&self, status: impl Into<String>) {
    self.lock().status = status.into();
    self.wake.send();
  }

  /// Set whether the spinner animates.
  pub fn set_busy(&self, busy: bool) {
    self.lock().busy = busy;
    self.wake.send();
  }

  /// Set the progress gauge and the discovered-test list (`View::Tests`).
  pub fn set_tests(&self, labels: Vec<String>) {
    let mut state = self.lock();
    state.tests = labels
      .into_iter()
      .map(|label| TestEntry {
        label,
        status: TestStatus::Pending,
      })
      .collect();
    state.progress_total = state.tests.len();
    drop(state);
    self.wake.send();
  }

  /// Set the progress gauge (`View::Tests`).
  pub fn set_progress(&self, done: usize, total: usize) {
    let mut state = self.lock();
    state.progress_done = done;
    state.progress_total = total;
    drop(state);
    self.wake.send();
  }

  /// Start buffering logs for `label` (`View::Tests`).
  pub fn begin_test(&self, label: impl Into<String>) {
    let mut state = self.lock();
    let label = label.into();
    if let Some(index) = state.test_index(&label)
      && let Some(test) = state.tests.get_mut(index)
    {
      test.status = TestStatus::Running;
    }
    state.current_test = Some(label);
    state.current_logs.clear();
    state.logs_scroll = 0;
    drop(state);
    self.wake.send();
  }

  /// Record a verdict; a failure keeps the test's buffered logs (`View::Tests`).
  pub fn verdict(
    &self,
    label: impl Into<String>,
    passed: bool,
    detail: Option<String>,
  ) {
    let mut state = self.lock();
    let label = label.into();
    if let Some(index) = state.test_index(&label)
      && let Some(test) = state.tests.get_mut(index)
    {
      test.status = if passed {
        TestStatus::Passed
      } else {
        TestStatus::Failed
      };
    }
    if !passed && let Some(detail) = detail {
      state.failure = Some(Failure {
        label,
        detail,
        logs: state.current_logs.clone(),
      });
    }
    drop(state);
    self.wake.send();
  }

  /// Clear the test view for a fresh `--watch` pass.
  pub fn reset(&self) {
    let mut state = self.lock();
    state.progress_done = 0;
    state.progress_total = 0;
    state.tests.clear();
    state.current_test = None;
    state.current_logs.clear();
    state.logs_scroll = 0;
    state.failure = None;
    drop(state);
    self.wake.send();
  }

  /// Take the receiver that resolves when the user quits the live view.
  ///
  /// Returns `None` once taken (or for a non-live run).
  pub fn take_quit_receiver(&self) -> Option<Receiver<()>> {
    self
      .quit
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .take()
  }
}

impl Drop for Live {
  fn drop(&mut self) {
    self.stop.store(true, Ordering::SeqCst);
    self.wake.send();
    if let Some(handle) = self
      .handle
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .take()
    {
      let _ = handle.join();
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn tab_index_reuses_and_creates_tabs() {
    let mut state = State::new(true);
    assert_eq!(state.tab_index("omw"), 0);
    assert_eq!(state.tab_index("mcp:a"), 1);
    assert_eq!(state.tab_index("mcp:a"), 1);
    assert_eq!(state.tab_index("mcp:b"), 2);
    assert_eq!(state.tabs.len(), 3);
  }

  #[test]
  fn push_line_caps_the_ring_buffer() {
    let mut state = State::new(true);
    for index in 0..DEFAULT_TAB_CAPACITY.saturating_add(5) {
      state.push_line("omw", index.to_string());
    }
    let tab = &state.tabs[0];
    assert_eq!(tab.lines.len(), DEFAULT_TAB_CAPACITY);
    assert_eq!(tab.lines.front().map(String::as_str), Some("5"));
  }

  #[test]
  fn push_line_honours_a_configured_cap() {
    let mut state = State::new(true);
    state.tab_capacity = Some(2);
    for index in 0..4 {
      state.push_line("omw", index.to_string());
    }
    let tab = &state.tabs[0];
    assert_eq!(tab.lines.len(), 2);
    assert_eq!(tab.lines.front().map(String::as_str), Some("2"));
  }

  #[test]
  fn push_line_with_no_cap_is_unbounded() {
    let mut state = State::new(true);
    state.tab_capacity = None;
    let pushes = DEFAULT_TAB_CAPACITY.saturating_add(5);
    for index in 0..pushes {
      state.push_line("omw", index.to_string());
    }
    assert_eq!(state.tabs[0].lines.len(), pushes);
  }

  #[test]
  fn defaults_match_the_documented_tunables() {
    let state = State::new(true);
    assert_eq!(state.tick, Duration::from_millis(80));
    assert_eq!(state.tab_capacity, Some(2000));
  }

  #[test]
  fn configure_updates_tick_and_capacity() {
    let state = Arc::new(Mutex::new(State::new(true)));
    let (wake_tx, _wake_rx) = std::sync::mpsc::channel();
    let (_quit_tx, quit_rx) = std::sync::mpsc::channel();
    let live = Live::new(
      Arc::clone(&state),
      Wake::new(wake_tx),
      quit_rx,
      Arc::new(AtomicBool::new(false)),
      std::thread::spawn(|| {}),
    );
    live.configure(500, 10);
    let guard = state.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(guard.tick, Duration::from_millis(500));
    assert_eq!(guard.tab_capacity, Some(10));
    drop(guard);
    live.configure(80, 0);
    let guard = state.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(guard.tab_capacity, None);
  }
}
