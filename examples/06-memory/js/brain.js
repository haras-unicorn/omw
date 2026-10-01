// The `[memory.alice]` table seeds `state` before the brain runs, so the agent
// starts already fast-forwarded to a state. `memoryGetAs` parses the stored
// JSON back into an object; the brain reads the model name off it.
const provider = omw.provider.get("openai");
const state = omw.host.memoryGetAs("state");
const model = state === undefined ? "fallback" : state.model;
const reply = provider.chat(model, [{ role: "user", content: "hi" }], []);
omw.host.log("info", reply.content);
