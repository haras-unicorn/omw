// Subscribe under a model, do a warm-up chat, then answer the one inbound
// request. The mock client's `after` gate makes the request arrive only after
// the `chat` call, which is exactly what the assertions pin down.
const subscription = omw.host.subscribeEndpoint("gpt-4o");
const provider = omw.provider.get("openai");
const reply = provider.chat(
  "gpt-test",
  [{ role: "user", content: "warm up" }],
  [],
);
const event = omw.host.recv();
const session = event.payload.session;
omw.host.streamEndpoint(session, { content: reply.content });
omw.host.streamEndpoint(session, { finish_reason: "stop" });
omw.host.unsubscribeEndpoint(subscription);
omw.host.log("info", "done");
