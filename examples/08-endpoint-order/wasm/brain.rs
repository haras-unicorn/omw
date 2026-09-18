// Subscribe under a model, do a warm-up chat, then answer the one inbound
// request. The mock client's `after` gate makes the request arrive only after
// the `chat` call, which is exactly what the assertions pin down.
#![no_main]

use omw_wasm_rust::{host, prelude::*};

omw_wasm_rust::brain!(|| {
  let _subscription = host::subscribe_endpoint("gpt-4o")?;
  let provider = Provider::get("openai")?;
  let reply =
    provider.chat("gpt-test", &[ChatMessage::user("warm up")], &[])?;
  let event = host::recv()?;
  let Event::EndpointMessage(message) = event.event else {
    return Err("expected an endpoint-message event".to_string());
  };
  host::stream_endpoint(
    &message.session,
    &ChatDelta::text(reply.text().unwrap_or_default()),
  )?;
  host::stream_endpoint(&message.session, &ChatDelta::finish("stop"))?;
  host::info("done");
  Ok(())
});
