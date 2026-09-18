// A ReAct loop: ask the model, run the tool it asks for, then send the tool
// result back for a final answer.
#![no_main]

use omw_wasm_rust::{host, prelude::*};

omw_wasm_rust::brain!(|| {
  let provider = Provider::get("openai")?;
  let tooling = Tooling::get("mcp")?;
  let first =
    provider.chat("gpt-test", &[ChatMessage::user("use echo")], &[])?;
  let call = first
    .tool_calls
    .into_iter()
    .next()
    .ok_or_else(|| "the model returned no tool call".to_string())?;
  let result = tooling.call_tool_blocking(&call.name, r#"{"input":"hi"}"#)?;
  let second = provider.chat(
    "gpt-test",
    &[
      ChatMessage::user("use echo"),
      ChatMessage::assistant("").with_tool_call(call),
      ChatMessage {
        role: Role::Tool,
        content: Some(result.value),
        tool_call: None,
      },
    ],
    &[],
  )?;
  host::info(second.text().unwrap_or_default());
  Ok(())
});
