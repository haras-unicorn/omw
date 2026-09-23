// The `[memory.alice]` table seeds `handle` before the brain runs, so the agent
// starts already fast-forwarded to a state. It reads the value and uses it as
// the model name; without the seed it would fall back.
#![no_main]

use omw_wasm_rust::{host, prelude::*};

omw_wasm_rust::brain!(|| {
  let provider = Provider::get("openai")?;
  let model =
    host::memory_get("handle").unwrap_or_else(|| "fallback".to_string());
  let reply = provider.chat(&model, &[ChatMessage::user("hi")], &[])?;
  host::info(reply.text().unwrap_or_default());
  Ok(())
});
