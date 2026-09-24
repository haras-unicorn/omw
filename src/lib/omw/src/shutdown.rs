//! Process-level shutdown latch: one OS signal subscription per
//! process, awaited by every agent iteration, the endpoint server, and
//! the `loop` backoff. Agents observe it through the existing
//! cooperative tiers (`recv` slices, `block_on_reload`, grace +
//! interrupt); this latch only decides *when* the supervisor asks them
//! to stop.

use tokio::sync::watch;

/// A process-wide shutdown request. Cloning shares the same latch;
/// the request is idempotent and [`wait`](Self::wait) resolves
/// immediately when already set, so iterations starting after the
/// signal still observe it.
#[derive(Debug, Clone)]
pub struct Shutdown {
  tx: watch::Sender<bool>,
}

impl Shutdown {
  pub fn new() -> Self {
    let (tx, _) = watch::channel(false);
    Self { tx }
  }

  /// Request shutdown. Idempotent and never blocks.
  ///
  /// Uses `send_replace` rather than `send`: a `watch` channel with no live
  /// receivers drops `send`, so a shutdown requested before any agent (or the
  /// endpoint) subscribed would be lost. `send_replace` stores the value
  /// regardless, so a later [`wait`](Self::wait) still observes it.
  pub fn request(&self) {
    self.tx.send_replace(true);
  }

  pub fn is_requested(&self) -> bool {
    *self.tx.borrow()
  }

  /// Resolve once shutdown is requested; returns immediately when
  /// already set. Each call subscribes afresh, so sharing one
  /// `Shutdown` across agents and iterations needs no extra
  /// bookkeeping.
  pub async fn wait(&self) {
    let mut rx = self.tx.subscribe();
    if *rx.borrow() {
      return;
    }
    let _ = rx.wait_for(|requested| *requested).await;
  }
}

impl Default for Shutdown {
  fn default() -> Self {
    Self::new()
  }
}

/// Resolve on SIGTERM/SIGINT so the supervisor can shut agents down
/// terminally. Pending forever when no signal arrives.
pub(crate) async fn shutdown_signal() {
  #[cfg(unix)]
  {
    let term =
      tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
    let int =
      tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt());
    let (Ok(mut term), Ok(mut int)) = (term, int) else {
      std::future::pending::<()>().await;
      return;
    };
    tokio::select! {
      _ = term.recv() => {},
      _ = int.recv() => {},
    }
  }
  #[cfg(not(unix))]
  {
    let _ = tokio::signal::ctrl_c().await;
  }
}
