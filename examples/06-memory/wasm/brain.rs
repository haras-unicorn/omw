// The `[memory.alice]` table seeds `state` before the brain runs, so the agent
// starts already fast-forwarded to a state. `memory_get_as` deserializes the
// stored JSON into `State`; the brain reads the model name off it.
#![no_main]

use omw_wasm_rust::{host, prelude::*};
use serde::Deserialize;

#[derive(Deserialize)]
struct State {
  model: String,
}

omw_wasm_rust::brain!(|| {
  let provider = Provider::get("openai")?;
  let model = match host::memory_get_as::<State>("state") {
    Some(Ok(state)) => state.model,
    _ => "fallback".to_string(),
  };
  let reply = provider.chat(&model, &[ChatMessage::user("hi")], &[], None)?;
  host::info(reply.text().unwrap_or_default());
  Ok(())
});
