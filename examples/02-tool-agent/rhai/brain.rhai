// A ReAct loop: ask the model, run the tool it asks for, then send the tool
// result back for a final answer.
let provider = omw::provider::get("openai");
let tooling = omw::tooling::get("mcp");
let first = provider.chat(
  "gpt-test",
  [#{ role: "user", content: "use echo" }],
  []
);
let invocation = first.tool_calls[0];
let result = tooling.call_tool_blocking(invocation.name, #{ input: "hi" });
let second = provider.chat(
  "gpt-test",
  [
    #{ role: "user", content: "use echo" },
    #{ role: "assistant", tool_call: invocation },
    #{ role: "tool", content: result.value },
  ],
  []
);
omw::host::log("info", second.content);
