//! Testing a config through the library: drive the in-config `kind = "mock"`
//! doubles with `omw::testing::Harness` and check the result.
//!
//! `Harness` runs the config through the controlled path, consumes the trace
//! as it happens, stops `outcome = "asserted"` agents as their assertions
//! settle, and returns a per-agent `Report`. The `omw-test` binary is a thin
//! CLI over exactly this.

use omw::prelude::*;
use omw::testing::{Harness, parse};

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
      outcome = "asserted"
      events = [
        { kind = "call", op = "chat", detail = { model = "mock" } },
      ]
    "#
  .replace("__SCRIPT__", &script.to_string_lossy());

  let config: Config = toml::from_str(&raw)?;
  let assertions = parse(&raw)?;
  let registries = Registries::default();

  let report = Harness::new(&config, &registries, &assertions).run().await;
  anyhow::ensure!(report.passed(), "harness reported failures: {report:?}");
  println!("harness passed: {} agent(s)", report.agents.len());
  Ok(())
}
