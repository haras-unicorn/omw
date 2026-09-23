// An endpoint agent: subscribe under a model name, then answer each inbound
// request with a blocking chat and a two-delta reply (content plus a terminal
// finish reason). The mock endpoint client scripts two requests.
const subscription = omw.host.subscribeEndpoint("gpt-4o");
const provider = omw.provider.get("openai");
for (let i = 0; i < 2; i += 1) {
  const event = omw.host.recv();
  const session = event.payload.session;
  const reply = provider.chat("gpt-test", event.payload.messages, []);
  omw.host.streamEndpoint(session, { content: reply.content });
  omw.host.streamEndpoint(session, { finish_reason: "stop" });
}
omw.host.unsubscribeEndpoint(subscription);
omw.host.log("info", "done");
