# The `[memory.alice]` table seeds `state` before the brain runs, so the agent
# starts already fast-forwarded to a state. `memory_get_as` parses the stored
# JSON back into an object; the brain reads the model name off it.
provider = omw.provider.get("openai")
state = omw.host.memory_get_as("state")
model = "fallback" if state is None else state.model
reply = provider.chat(model, [{"role": "user", "content": "hi"}], [])
omw.host.log("info", reply.content)
