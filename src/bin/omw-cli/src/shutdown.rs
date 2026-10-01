//! OS signal subscription: the process-policy half of shutdown.
//!
//! The `omw` library owns the cooperative [`Shutdown`] latch but installs no
//! signal handler (see `omw::shutdown`); subscribing to SIGTERM/SIGINT is the
//! binary's job, alongside `log` and `tls`.

use omw::shutdown::Shutdown;

/// Request `shutdown` on the first SIGTERM/SIGINT (Ctrl-C off unix). Returns
/// the spawned task so the caller can abort it once the run is over.
pub fn install(shutdown: &Shutdown) -> tokio::task::JoinHandle<()> {
  let shutdown = shutdown.clone();
  tokio::spawn(async move {
    signal().await;
    tracing::info!("shutdown signal received");
    shutdown.request();
  })
}

/// Resolve on SIGTERM/SIGINT; pending forever when no signal arrives.
async fn signal() {
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
