// The `[memory.alice]` table seeds `state` before the brain runs, so the agent
// starts already fast-forwarded to a state. `memory_get_as` parses the stored
// JSON back into a map; the brain reads the model name off it.
let provider = omw::provider::get("openai");
let state = omw::host::memory_get_as("state");
let model = if state == () { "fallback" } else { state.model };
let reply = provider.chat(model, [#{ role: "user", content: "hi" }], []);
omw::host::log("info", reply.content);
