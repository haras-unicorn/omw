// 10-endpoint-stop: the endpoint mock ends each session from the client side.
// We stream one content delta per request and then wait for the session to end
// — a normal stop for the first, an abort for the second — instead of sending a
// terminal finish reason ourselves.
#![no_main]

use omw_wasm_rust::{host, prelude::*};

omw_wasm_rust::brain!(|| {
  let _subscription = host::subscribe_endpoint("gpt-4o")?;
  let provider = Provider::get("openai")?;
  for _ in 0..2 {
    let request = host::recv()?;
    let Event::EndpointMessage(message) = request.event else {
      return Err("expected an endpoint-message event".to_string());
    };
    let reply = provider.chat("gpt-test", &message.messages, &[], None)?;
    host::stream_endpoint(
      &message.session,
      &ChatDelta::text(reply.content.unwrap_or_default()),
    )?;
    let ended = host::recv()?;
    let kind = ended.kind();
    let Event::EndpointSessionEnd(end) = ended.event else {
      return Err("expected an endpoint-session-end event".to_string());
    };
    host::info(&format!(
      "{kind}{}",
      end.error.map(|error| format!("|{error}")).unwrap_or_default(),
    ));
  }
  host::info("done");
  Ok(())
});
