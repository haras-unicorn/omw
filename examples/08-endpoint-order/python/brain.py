# Subscribe under a model, do a warm-up chat, then answer the one inbound
# request. The mock client's `after` gate makes the request arrive only after
# the `chat` call, which is exactly what the assertions pin down.
subscription = omw.host.subscribe_endpoint("gpt-4o")
provider = omw.provider.get("openai")
reply = provider.chat("gpt-test", [{"role": "user", "content": "warm up"}], [])
event = omw.host.recv()
session = event.payload.session
omw.host.stream_endpoint(session, {"content": reply.content})
omw.host.stream_endpoint(session, {"finish_reason": "stop"})
omw.host.unsubscribe_endpoint(subscription)
omw.host.log("info", "done")
