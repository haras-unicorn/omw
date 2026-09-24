// Four blocking chats with different model names. The provider script has
// three turns, so the fourth chat repeats the last turn. Only the calls matter
// here: 05-patterns asserts them with regex leaves, `$while`, and `$until`.
#![no_main]

use omw_wasm_rust::{host, prelude::*};

omw_wasm_rust::brain!(|| {
  let provider = Provider::get("openai")?;
  let models = ["gpt-alpha", "gpt-beta", "gpt-gamma", "gpt-alpha"];
  let mut last = String::new();
  for model in models {
    let reply = provider.chat(model, &[ChatMessage::user("hi")], &[])?;
    last = reply.text().unwrap_or_default().to_string();
  }
  host::info(&last);
  Ok(())
});
