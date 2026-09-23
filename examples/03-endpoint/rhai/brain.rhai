// An endpoint agent: subscribe under a model name, then answer each inbound
// request with a blocking chat and a two-delta reply (content plus a terminal
// finish reason). The mock endpoint client scripts two requests.
let subscription = omw::host::subscribe_endpoint("gpt-4o");
let provider = omw::provider::get("openai");
for i in 0..2 {
  let event = omw::host::recv();
  let session = event.payload.session;
  let reply = provider.chat("gpt-test", event.payload.messages, []);
  omw::host::stream_endpoint(session, #{ content: reply.content });
  omw::host::stream_endpoint(session, #{ finish_reason: "stop" });
}
omw::host::unsubscribe_endpoint(subscription);
omw::host::log("info", "done");
