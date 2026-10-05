# A blocking chat: the full reply comes back in-band, so the only event the
# trace records is the `chat` call itself. `log` returns None, so the run
# reports `completed` instead of exiting with the reply text.
provider = omw.provider.get("openai")
messages = [{"role": "user", "content": "say hi"}]
reply = provider.chat("gpt-test", messages, [])
omw.host.log("info", reply.content)
