//! The ratatui render thread: alternate screen, raw mode and drawing.
//!
//! Setup runs on the caller's thread so a failure (for example, stderr is not a
//! terminal) can fall back to `pipe` synchronously. The loop then owns the
//! terminal and redraws on wake, on input, and on a short timeout that drives
//! the spinner.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossterm::event::{
  self, Event as CtEvent, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::terminal::{
  EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{
  Block, BorderType, Gauge, List, ListItem, ListState, Paragraph,
};
use ratatui::{Frame, Terminal};

use crate::live::{State, TestStatus, View};

/// Spinner frames shown while the run is busy.
const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

/// Key hints shown on the right of the bottom line.
const AGENTS_HINTS: &str = "←/→ h/l source  ↑/↓ j/k scroll  q quit";
const TESTS_HINTS: &str = "↑/↓ j/k scroll  q quit";

/// Set up the terminal on `stderr` and spawn the render thread.
///
/// Returns an error (without leaving the terminal changed) when raw mode or
/// the alternate screen cannot be entered, so the caller can fall back to
/// `pipe`.
pub(crate) fn start(
  state: Arc<Mutex<State>>,
  wake: Receiver<()>,
  quit: Sender<()>,
  stop: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
  let backend = CrosstermBackend::new(io::stderr());
  let mut terminal = Terminal::new(backend)?;
  enable_raw_mode()?;
  if let Err(error) = execute!(io::stderr(), EnterAlternateScreen, cursor::Hide)
  {
    let _ = disable_raw_mode();
    return Err(error);
  }
  install_panic_hook();

  thread::Builder::new()
    .name("omw-tui".to_owned())
    .spawn(move || render_loop(&mut terminal, state, wake, quit, stop))
}

/// Restore the terminal if the process panics while the live view is up.
fn install_panic_hook() {
  static INSTALLED: AtomicBool = AtomicBool::new(false);
  if INSTALLED.swap(true, Ordering::SeqCst) {
    return;
  }
  let previous = std::panic::take_hook();
  std::panic::set_hook(Box::new(move |info| {
    teardown();
    previous(info);
  }));
}

/// Leave the alternate screen and raw mode.
fn teardown() {
  let _ = disable_raw_mode();
  let _ = execute!(io::stderr(), LeaveAlternateScreen, cursor::Show);
}

fn render_loop(
  terminal: &mut Terminal<CrosstermBackend<io::Stderr>>,
  state: Arc<Mutex<State>>,
  wake: Receiver<()>,
  quit: Sender<()>,
  stop: Arc<AtomicBool>,
) {
  let mut quitting = false;
  while !quitting && !stop.load(Ordering::SeqCst) {
    let tick = {
      let guard = state.lock().unwrap_or_else(PoisonError::into_inner);
      let _ = terminal.draw(|frame| draw(frame, &guard));
      guard.tick
    };

    let _ = wake.recv_timeout(tick);

    while let Ok(true) = event::poll(Duration::ZERO) {
      match event::read() {
        Ok(CtEvent::Key(key)) => handle_key(key, &state, &mut quitting),
        Ok(_) => {}
        Err(_) => break,
      }
    }

    if !quitting {
      let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
      guard.spinner = guard.spinner.wrapping_add(1);
    }
  }

  teardown();
  if quitting {
    let _ = quit.send(());
  }
}

fn handle_key(key: KeyEvent, state: &Arc<Mutex<State>>, quitting: &mut bool) {
  if key.modifiers.contains(KeyModifiers::CONTROL)
    && key.code == KeyCode::Char('c')
  {
    *quitting = true;
    return;
  }
  match key.code {
    KeyCode::Char('q') | KeyCode::Esc => *quitting = true,
    KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => cycle(state, 1),
    KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => cycle(state, -1),
    KeyCode::Up | KeyCode::Char('k') => scroll(state, 1),
    KeyCode::Down | KeyCode::Char('j') => scroll(state, -1),
    _ => {}
  }
}

/// Move the selected source tab by `delta`, wrapping (`View::Agents` only).
fn cycle(state: &Arc<Mutex<State>>, delta: isize) {
  let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
  if guard.view != View::Agents {
    return;
  }
  let len = guard.tabs.len();
  if len == 0 {
    return;
  }
  let active = guard.active as isize;
  let next = active.wrapping_add(delta).rem_euclid(len as isize);
  guard.active = next as usize;
}

/// Scroll the active pane by `delta` lines (positive scrolls up).
fn scroll(state: &Arc<Mutex<State>>, delta: isize) {
  let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
  match guard.view {
    View::Agents => {
      let active = guard.active;
      if let Some(tab) = guard.tabs.get_mut(active) {
        tab.scroll = tab.scroll.saturating_add_signed(delta);
      }
    }
    View::Tests => {
      guard.logs_scroll = guard.logs_scroll.saturating_add_signed(delta);
    }
  }
}

fn draw(frame: &mut Frame<'_>, state: &State) {
  match state.view {
    View::Agents => draw_agents(frame, state),
    View::Tests => draw_tests(frame, state),
  }
}

fn draw_agents(frame: &mut Frame<'_>, state: &State) {
  let chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
      Constraint::Length(4),
      Constraint::Min(0),
      Constraint::Length(1),
    ])
    .split(frame.area());

  draw_info(frame, chunks[0], state, "omw");

  let body = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Length(24), Constraint::Min(0)])
    .split(chunks[1]);

  let items: Vec<ListItem> = state
    .tabs
    .iter()
    .map(|tab| ListItem::new(tab.name.clone()))
    .collect();
  let list = List::new(items)
    .block(pane("Sources", state.color))
    .highlight_style(highlight(state.color));
  let mut list_state = ListState::default();
  list_state.select(Some(state.active));
  frame.render_stateful_widget(list, body[0], &mut list_state);

  if let Some(tab) = state.tabs.get(state.active) {
    let visible = inner_height(body[1]);
    let total = tab.lines.len();
    let end = total.saturating_sub(tab.scroll);
    let start = end.saturating_sub(visible);
    let lines: Vec<Line> = tab
      .lines
      .range(start..end)
      .map(|line| Line::styled(line.clone(), line_style(state.color, line)))
      .collect();
    let paragraph =
      Paragraph::new(Text::from(lines)).block(pane(&tab.name, state.color));
    frame.render_widget(paragraph, body[1]);
  }

  draw_status(frame, chunks[2], state, AGENTS_HINTS);
}

fn draw_tests(frame: &mut Frame<'_>, state: &State) {
  let chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
      Constraint::Length(4),
      Constraint::Min(0),
      Constraint::Length(3),
      Constraint::Length(1),
    ])
    .split(frame.area());

  draw_info(frame, chunks[0], state, "omw-test");

  let body = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Length(28), Constraint::Min(0)])
    .split(chunks[1]);

  let spinner = spinner(state);
  let items: Vec<ListItem> = state
    .tests
    .iter()
    .map(|test| {
      let (mark, style) = test_mark(test.status, state.color, spinner);
      ListItem::new(Line::styled(format!("{mark} {}", test.label), style))
    })
    .collect();
  let list = List::new(items).block(pane("Tests", state.color));
  frame.render_widget(list, body[0]);

  let content = test_logs(state);
  let visible = inner_height(body[1]);
  let total = content.len();
  let end = total.saturating_sub(state.logs_scroll);
  let start = end.saturating_sub(visible);
  let lines: Vec<Line> = content[start..end]
    .iter()
    .map(|line| Line::styled(line.clone(), line_style(state.color, line)))
    .collect();
  let paragraph =
    Paragraph::new(Text::from(lines)).block(pane("Logs", state.color));
  frame.render_widget(paragraph, body[1]);

  draw_gauge(frame, chunks[2], state);
  draw_status(frame, chunks[3], state, TESTS_HINTS);
}

/// The bordered info panel: title, the launched command, and the details.
fn draw_info(frame: &mut Frame<'_>, area: Rect, state: &State, title: &str) {
  let command = Line::from(format!("$ {}", state.command));
  let details = state
    .details
    .iter()
    .map(|(key, value)| format!("{key}: {value}"))
    .collect::<Vec<_>>()
    .join(" · ");
  let paragraph =
    Paragraph::new(Text::from(vec![command, Line::from(details)]))
      .block(pane(title, state.color));
  frame.render_widget(paragraph, area);
}

fn draw_gauge(frame: &mut Frame<'_>, area: Rect, state: &State) {
  let ratio = if state.progress_total == 0 {
    0.0
  } else {
    state.progress_done as f64 / state.progress_total as f64
  };
  let gauge_style = if state.color {
    Style::default().fg(Color::Cyan)
  } else {
    Style::default()
  };
  let gauge = Gauge::default()
    .block(pane("", state.color))
    .gauge_style(gauge_style)
    .ratio(ratio.clamp(0.0, 1.0))
    .label(format!(
      "{}/{} tests",
      state.progress_done, state.progress_total
    ));
  frame.render_widget(gauge, area);
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, state: &State, hints: &str) {
  let spinner = spinner(state);
  let status = format!("{spinner} {}", state.status);
  let width = u16::try_from(hints.chars().count()).unwrap_or(u16::MAX);
  let chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Min(0), Constraint::Length(width)])
    .split(area);
  frame.render_widget(Paragraph::new(Line::from(status)), chunks[0]);
  let hints = Line::from(hints).alignment(Alignment::Right);
  frame.render_widget(Paragraph::new(hints), chunks[1]);
}

/// The spinner frame for the current state, or a blank when idle.
fn spinner(state: &State) -> &'static str {
  if state.busy {
    SPINNER[state.spinner.checked_rem(SPINNER.len()).unwrap_or(0)]
  } else {
    " "
  }
}

/// The status mark and style for one test in the list.
fn test_mark(
  status: TestStatus,
  color: bool,
  spinner: &str,
) -> (String, Style) {
  match status {
    TestStatus::Passed => ("✅".to_owned(), color_style(color, Color::Green)),
    TestStatus::Failed => ("❌".to_owned(), color_style(color, Color::Red)),
    TestStatus::Running => (spinner.to_owned(), Style::default()),
    TestStatus::Pending => (" ".to_owned(), Style::default()),
  }
}

/// The lines the `View::Tests` logs pane shows: the failure diff and its
/// buffered logs when one is present, otherwise the running test's logs.
fn test_logs(state: &State) -> Vec<String> {
  if let Some(failure) = &state.failure {
    let mut lines = vec![format!("FAIL {}", failure.label)];
    for line in failure.detail.lines() {
      lines.push(format!("  {line}"));
    }
    if !failure.logs.is_empty() {
      lines.push(String::new());
      lines.push("logs:".to_owned());
      for line in &failure.logs {
        lines.push(format!("  {line}"));
      }
    }
    return lines;
  }
  state.current_logs()
}

/// A rounded, single-line bordered pane.
fn pane<'a>(title: &'a str, _color: bool) -> Block<'a> {
  Block::bordered()
    .border_type(BorderType::Rounded)
    .title(title)
}

/// The highlight style for a selected list row.
fn highlight(color: bool) -> Style {
  if color {
    Style::default().fg(Color::Black).bg(Color::Cyan)
  } else {
    Style::default().add_modifier(Modifier::REVERSED)
  }
}

fn color_style(color: bool, fg: Color) -> Style {
  if color {
    Style::default().fg(fg)
  } else {
    Style::default()
  }
}

/// The usable height inside a bordered pane.
fn inner_height(area: Rect) -> usize {
  usize::from(area.height.saturating_sub(2))
}

/// Color a log line by its level prefix; monochrome when `color` is false.
fn line_style(color: bool, line: &str) -> Style {
  if !color {
    return Style::default();
  }
  match line.split_whitespace().next().unwrap_or_default() {
    "ERROR" => Style::default().fg(Color::Red),
    "WARN" => Style::default().fg(Color::Yellow),
    "INFO" => Style::default().fg(Color::Cyan),
    "DEBUG" | "TRACE" => Style::default().fg(Color::DarkGray),
    _ => Style::default(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::live::Failure;
  use ratatui::backend::TestBackend;

  fn render(state: &State) -> anyhow::Result<String> {
    let mut terminal = Terminal::new(TestBackend::new(70, 16))?;
    terminal.draw(|frame| draw(frame, state))?;
    Ok(format!("{}", terminal.backend()))
  }

  #[test]
  fn line_style_is_monochrome_without_color() {
    assert_eq!(line_style(false, "ERROR omw: boom"), Style::default());
  }

  fn lock(state: &Arc<Mutex<State>>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
  }

  fn press(state: &Arc<Mutex<State>>, code: KeyCode) {
    let mut quitting = false;
    handle_key(
      KeyEvent::new(code, KeyModifiers::NONE),
      state,
      &mut quitting,
    );
    assert!(!quitting);
  }

  #[test]
  fn h_and_l_cycle_agents_sources() {
    let state = Arc::new(Mutex::new(State::new(false)));
    lock(&state).push_line("mcp:a", "x".to_owned());
    press(&state, KeyCode::Char('l'));
    assert_eq!(lock(&state).active, 1);
    press(&state, KeyCode::Char('h'));
    assert_eq!(lock(&state).active, 0);
  }

  #[test]
  fn j_and_k_scroll_the_agents_source() {
    let state = Arc::new(Mutex::new(State::new(false)));
    press(&state, KeyCode::Char('k'));
    assert_eq!(lock(&state).tabs[0].scroll, 1);
    press(&state, KeyCode::Char('j'));
    assert_eq!(lock(&state).tabs[0].scroll, 0);
  }

  #[test]
  fn j_and_k_scroll_the_tests_logs() {
    let state = Arc::new(Mutex::new(State::new(false)));
    lock(&state).view = View::Tests;
    press(&state, KeyCode::Char('k'));
    assert_eq!(lock(&state).logs_scroll, 1);
    press(&state, KeyCode::Char('j'));
    assert_eq!(lock(&state).logs_scroll, 0);
    press(&state, KeyCode::Char('l'));
    assert_eq!(lock(&state).active, 0);
  }

  #[test]
  fn line_style_colors_by_level() {
    assert_eq!(
      line_style(true, "ERROR omw: boom"),
      Style::default().fg(Color::Red)
    );
    assert_eq!(
      line_style(true, " INFO omw: ok"),
      Style::default().fg(Color::Cyan)
    );
  }

  #[test]
  fn agents_view_draws_command_sources_logs_and_status() -> anyhow::Result<()> {
    let mut state = State::new(false);
    state.command = "target/debug/omw run --config omw.toml".to_owned();
    state.details = vec![
      ("config".to_owned(), "omw.toml".to_owned()),
      ("agents".to_owned(), "1".to_owned()),
    ];
    state.push_line("omw", " INFO omw: hello".to_owned());
    state.push_line("mcp:everything", " INFO mcp: world".to_owned());
    state.busy = true;
    state.status = "1 agent(s) running".to_owned();
    let rendered = render(&state)?;
    assert!(rendered.contains("$ target/debug/omw run"), "{rendered}");
    assert!(rendered.contains("config: omw.toml"), "{rendered}");
    assert!(rendered.contains("Sources"), "{rendered}");
    assert!(rendered.contains("hello"), "{rendered}");
    assert!(rendered.contains("1 agent(s) running"), "{rendered}");
    assert!(rendered.contains("q quit"), "{rendered}");
    Ok(())
  }

  #[test]
  fn tests_view_draws_command_marks_gauge_and_failure() -> anyhow::Result<()> {
    let mut state = State::new(false);
    state.view = View::Tests;
    state.command = "target/debug/omw-test run .".to_owned();
    state.details = vec![("path".to_owned(), ".".to_owned())];
    state.tests = vec![
      crate::live::TestEntry {
        label: "a".to_owned(),
        status: TestStatus::Passed,
      },
      crate::live::TestEntry {
        label: "b".to_owned(),
        status: TestStatus::Failed,
      },
    ];
    state.progress_done = 2;
    state.progress_total = 2;
    state.failure = Some(Failure {
      label: "b".to_owned(),
      detail: "agent \"x\" failed:\n  not satisfied".to_owned(),
      logs: vec![" INFO omw: hi".to_owned()],
    });
    let rendered = render(&state)?;
    assert!(
      rendered.contains("$ target/debug/omw-test run"),
      "{rendered}"
    );
    assert!(rendered.contains("✅"), "{rendered}");
    assert!(rendered.contains("❌"), "{rendered}");
    assert!(rendered.contains("2/2 tests"), "{rendered}");
    assert!(rendered.contains("not satisfied"), "{rendered}");
    assert!(rendered.contains("logs:"), "{rendered}");
    Ok(())
  }
}
