// An endpoint agent: subscribe under a model name, then answer each inbound
// request with a blocking chat and a two-delta reply (content plus a terminal
// finish reason). The mock endpoint client scripts two requests.
#![no_main]

use omw_wasm_rust::{host, prelude::*};

omw_wasm_rust::brain!(|| {
  let _subscription = host::subscribe_endpoint("gpt-4o")?;
  let provider = Provider::get("openai")?;
  for _ in 0..2 {
    let event = host::recv()?;
    let Event::EndpointMessage(message) = event.event else {
      return Err("expected an endpoint-message event".to_string());
    };
    let reply = provider.chat("gpt-test", &message.messages, &[])?;
    host::stream_endpoint(
      &message.session,
      &ChatDelta::text(reply.content.unwrap_or_default()),
    )?;
    host::stream_endpoint(&message.session, &ChatDelta::finish("stop"))?;
  }
  host::info("done");
  Ok(())
});
