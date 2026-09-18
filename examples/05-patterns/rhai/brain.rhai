// Four blocking chats with different model names. The provider script has
// three turns, so the fourth chat repeats the last turn. Only the calls matter
// here: 05-patterns asserts them with regex leaves, `$any`, and `$skip`.
let provider = omw::provider::get("openai");
let models = ["gpt-alpha", "gpt-beta", "gpt-gamma", "gpt-alpha"];
let last = "";
for model in models {
  let reply = provider.chat(model, [#{ role: "user", content: "hi" }], []);
  last = reply.content;
}
omw::host::log("info", last);
