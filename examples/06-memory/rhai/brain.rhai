// The `[memory.alice]` table seeds `handle` before the brain runs, so the agent
// starts already fast-forwarded to a state. It reads the value and uses it as
// the model name; without the seed it would fall back.
let provider = omw::provider::get("openai");
let handle = omw::host::memory_get("handle");
let model = if handle == () { "fallback" } else { handle };
let reply = provider.chat(model, [#{ role: "user", content: "hi" }], []);
omw::host::log("info", reply.content);
