// The `[memory.alice]` table seeds `handle` before the brain runs, so the agent
// starts already fast-forwarded to a state. It reads the value and uses it as
// the model name; without the seed it would fall back.
const provider = omw.provider.get("openai");
const handle = omw.host.memoryGet("handle");
const model = handle === undefined ? "fallback" : handle;
const reply = provider.chat(model, [{ role: "user", content: "hi" }], []);
omw.host.log("info", reply.content);
