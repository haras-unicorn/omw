// 10-endpoint-stop: the endpoint mock ends each session from the client side.
// We stream one content delta per request and then wait for the session to end
// — a normal stop for the first, an abort for the second — instead of sending a
// terminal finish reason ourselves.
const subscription = omw.host.subscribeEndpoint("gpt-4o");
const provider = omw.provider.get("openai");
for (let i = 0; i < 2; i += 1) {
  const request = omw.host.recv();
  const session = request.payload.session;
  const reply = provider.chat("gpt-test", request.payload.messages, []);
  omw.host.streamEndpoint(session, { content: reply.content });
  const ended = omw.host.recv();
  omw.host.log("info", ended.kind);
}
omw.host.unsubscribeEndpoint(subscription);
omw.host.log("info", "done");
