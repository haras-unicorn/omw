// A blocking chat: the full reply comes back in-band, so the only event the
// trace records is the `chat` call itself. `log` returns undefined, so the
// run reports `completed` instead of exiting with the reply text.
const provider = omw.provider.get("openai");
const messages = [{ role: "user", content: "say hi" }];
const reply = provider.chat("gpt-test", messages, []);
omw.host.log("info", reply.content);
