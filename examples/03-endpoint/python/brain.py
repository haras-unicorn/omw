# An endpoint agent: subscribe under a model name, then answer each inbound
# request with a blocking chat and a two-delta reply (content plus a terminal
# finish reason). The mock endpoint client scripts two requests.
subscription = omw.host.subscribe_endpoint("gpt-4o")
provider = omw.provider.get("openai")
for _ in range(2):
  event = omw.host.recv()
  session = event.payload.session
  reply = provider.chat("gpt-test", event.payload.messages, [])
  omw.host.stream_endpoint(session, {"content": reply.content})
  omw.host.stream_endpoint(session, {"finish_reason": "stop"})
omw.host.unsubscribe_endpoint(subscription)
omw.host.log("info", "done")
