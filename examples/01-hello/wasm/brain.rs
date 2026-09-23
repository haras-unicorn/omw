// A blocking chat: the full reply comes back in-band, so the only event the
// trace records is the `chat` call itself. Returning `Ok(())` reports
// `completed` instead of exiting with the reply text.
#![no_main]

use omw_wasm_rust::{host, prelude::*};

omw_wasm_rust::brain!(|| {
  let provider = Provider::get("openai")?;
  let messages = [ChatMessage::user("say hi")];
  let reply = provider.chat("gpt-test", &messages, &[])?;
  host::info(reply.text().unwrap_or_default());
  Ok(())
});
