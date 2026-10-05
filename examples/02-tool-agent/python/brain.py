# A ReAct loop: ask the model, run the tool it asks for, then send the tool
# result back for a final answer.
provider = omw.provider.get("openai")
tooling = omw.tooling.get("mcp")
first = provider.chat("gpt-test", [{"role": "user", "content": "use echo"}], [])
call = first.tool_calls[0]
result = tooling.call_tool_blocking(call.name, {"input": "hi"})
second = provider.chat(
  "gpt-test",
  [
    {"role": "user", "content": "use echo"},
    {"role": "assistant", "tool_call": call},
    {"role": "tool", "content": result.content[0].text},
  ],
  [],
)
omw.host.log("info", second.content)
