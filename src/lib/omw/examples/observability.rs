//! Observability through the library: subscribe to the trace channel and
//! stream every event omw produces as it happens, then verify the recorded run.
//!
//! The trace is a plain `tokio::sync::broadcast` channel of [`TraceEvent`]s
//! (inbound events, host calls, and terminal outcomes, each tagged with the
//! agent). `run_agents_traced` also returns the collected events, so the same
//! stream powers both a live view (a logger, a UI) and post-hoc assertions via
//! [`omw::testing`].
//!
//! [`TraceEvent`]: omw::host::trace::TraceEvent

use omw::prelude::*;
use omw::testing::{check, collect, parse};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let dir = tempfile::tempdir()?;
  let script = dir.path().join("brain.rhai");
  std::fs::write(
    &script,
    r#"
      let provider = omw::provider::get("mock");
      let reply = provider.chat("mock", [#{ role: "user", content: "hi" }], []);
      omw::host::log("info", reply.content);
    "#,
  )?;

  let raw = r#"
      [providers.mock]
      kind = "mock"
      turns = [{ content = "hello" }]

      [runtime.rhai]
      kind = "rhai"

      [[agents]]
      name = "alice"
      runtime = "rhai"
      script = "__SCRIPT__"

      [assertions.alice]
      outcome = "completed"
      events = [
        { kind = "call", op = "chat", detail = { model = "mock" } },
      ]
    "#
  .replace("__SCRIPT__", &script.to_string_lossy());

  let config: Config = toml::from_str(&raw)?;
  let assertions = parse(&raw)?;
  let registries = Registries::default();

  // The live view: one task drains the trace channel and reports each event.
  let (tx, mut rx) =
    tokio::sync::broadcast::channel(config.tunables.trace_buffer);
  let observer = tokio::spawn(async move {
    while let Ok(event) = rx.recv().await {
      println!("trace: {event:?}");
    }
  });

  // `run_agents_traced` drives the run and returns everything observed.
  let events = run_agents_traced(&config, false, &registries, tx).await?;
  let _ = observer.await;
  println!("collected {} trace events", events.len());

  // The same events still verify against the config's `[assertions]`.
  check(&collect(events), &assertions)?;
  Ok(())
}
