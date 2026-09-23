// A ReAct loop: ask the model, run the tool it asks for, then send the tool
// result back for a final answer.
const provider = omw.provider.get("openai");
const tooling = omw.tooling.get("mcp");
const first = provider.chat(
  "gpt-test",
  [{ role: "user", content: "use echo" }],
  [],
);
const call = first.tool_calls[0];
const result = tooling.callToolBlocking(call.name, { input: "hi" });
const second = provider.chat(
  "gpt-test",
  [
    { role: "user", content: "use echo" },
    { role: "assistant", tool_call: call },
    { role: "tool", content: result.value },
  ],
  [],
);
omw.host.log("info", second.content);
