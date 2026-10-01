// A ReAct loop: ask the model, run the tool it asks for, then send the tool
// result back for a final answer.
const provider = omw.provider.get("openai");
const tooling = omw.tooling.get("mcp");
const first = provider.chat(
  "gpt-test",
  [{ role: "user", content: "use echo" }],
  [],
);
const call = first.toolCalls[0];
const result = tooling.callToolBlocking(call.name, { input: "hi" });
const second = provider.chat(
  "gpt-test",
  [
    { role: "user", content: "use echo" },
    { role: "assistant", toolCall: call },
    { role: "tool", content: result.content[0].text },
  ],
  [],
);
omw.host.log("info", second.content);
