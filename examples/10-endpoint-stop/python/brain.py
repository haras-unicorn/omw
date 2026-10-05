# 10-endpoint-stop: the endpoint mock ends each session from the client side.
# We stream one content delta per request and then wait for the session to end
# — a normal stop for the first, an abort for the second — instead of sending a
# terminal finish reason ourselves.
subscription = omw.host.subscribe_endpoint("gpt-4o")
provider = omw.provider.get("openai")
for _ in range(2):
  request = omw.host.recv()
  session = request.payload.session
  reply = provider.chat("gpt-test", request.payload.messages, [])
  omw.host.stream_endpoint(session, {"content": reply.content})
  ended = omw.host.recv()
  omw.host.log("info", ended.kind)
omw.host.unsubscribe_endpoint(subscription)
omw.host.log("info", "done")
