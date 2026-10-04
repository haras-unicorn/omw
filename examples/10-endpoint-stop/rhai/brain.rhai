// 10-endpoint-stop: the endpoint mock ends each session from the client side.
// We stream one content delta per request and then wait for the session to end
// — a normal stop for the first, an abort for the second — instead of sending a
// terminal finish reason ourselves.
let subscription = omw::host::subscribe_endpoint("gpt-4o");
let provider = omw::provider::get("openai");
for i in 0..2 {
  let request = omw::host::recv();
  let session = request.payload.session;
  let reply = provider.chat("gpt-test", request.payload.messages, []);
  omw::host::stream_endpoint(session, #{ content: reply.content });
  let ended = omw::host::recv();
  omw::host::log("info", ended.kind);
}
omw::host::unsubscribe_endpoint(subscription);
omw::host::log("info", "done");
