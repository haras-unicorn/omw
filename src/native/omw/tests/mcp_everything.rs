//! Integration test for the MCP tooling against the official
//! `@modelcontextprotocol/server-everything` reference implementation, run in
//! the `mcp/everything` container.
//!
//! The everything server is a *stdio* MCP server, so we connect by spawning the
//! container with `docker run -i` through the production stdio transport
//! (`MCPTooling::build` with `transport = "stdio"`). This exercises the full
//! `initialize` -> `tools/list` -> `tools/call` lifecycle against a fully
//! independent implementation of the wire protocol.
//!
//! Opt-in via `OMW_TEST_MCP_EVERYTHING`.

use serde_json::json;

use omw::tooling::build;

const EVERYTHING_IMAGE: &str = "mcp/everything";

fn enabled() -> bool {
  std::env::var_os("OMW_TEST_MCP_EVERYTHING").is_some_and(|value| value != "0")
}

/// Connect to the everything server over stdio via the production build path.
async fn everything_tooling() -> anyhow::Result<omw::tooling::ToolingEntry> {
  build(
    "everything",
    "mcp",
    &json!({
      "transport": "stdio",
      "command": "docker",
      "args": ["run", "-i", "--rm", EVERYTHING_IMAGE],
    }),
  )
  .await
}

#[tokio::test]
async fn lists_tools_from_reference_server() -> anyhow::Result<()> {
  if !enabled() {
    eprintln!("skipping: OMW_TEST_MCP_EVERYTHING not set");
    return Ok(());
  }

  let entry = everything_tooling().await?;
  let tools = entry.tooling.list_tools().await?;
  let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
  assert!(
    names.contains(&"echo"),
    "expected a `echo` tool, got {names:?}"
  );
  assert!(
    names.contains(&"add"),
    "expected an `add` tool, got {names:?}"
  );
  Ok(())
}

#[tokio::test]
async fn calls_add_tool_and_gets_the_sum() -> anyhow::Result<()> {
  if !enabled() {
    eprintln!("skipping: OMW_TEST_MCP_EVERYTHING not set");
    return Ok(());
  }

  let entry = everything_tooling().await?;
  let sum = entry
    .tooling
    .call_tool("add", json!({ "a": 2, "b": 3 }))
    .await?;
  assert_eq!(sum.trim(), "The sum of 2 and 3 is 5.");
  Ok(())
}
