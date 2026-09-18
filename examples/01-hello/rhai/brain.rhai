// A blocking chat: the full reply comes back in-band, so the only event the
// trace records is the `chat` call itself. The trailing `log` keeps the
// script's value unit, so the run reports `completed` instead of exiting with
// the reply text.
let provider = omw::provider::get("openai");
let messages = [#{ role: "user", content: "say hi" }];
let reply = provider.chat("gpt-test", messages, []);
omw::host::log("info", reply.content);
