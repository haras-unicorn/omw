//! Integration tests for the OpenAI-family provider against a hermetic
//! in-process HTTP server (wiremock) that speaks the SSE chat wire protocol.
//!
//! These pin our parsing/encoding over real HTTP: content deltas, tool-call
//! reassembly across fragmented chunks, error surfacing, and the outgoing
//! request (bearer token, model, streaming).

use std::time::Duration;

use futures_util::StreamExt;
use omw::provider::{ChatMessage, ProviderEntry, Role, ToolCall};
use serde_json::json;
use wiremock::matchers::{bearer_token, body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse_line(json: &str) -> String {
  format!("data: {json}\n\n")
}

/// Collect every delta `chat` yields for the given request.
async fn chat_all(
  entry: &ProviderEntry,
  model: &str,
  messages: Vec<ChatMessage>,
) -> anyhow::Result<Vec<omw::provider::ChatDelta>> {
  let mut stream = entry.provider.chat(model, messages, Vec::new()).await?;
  let mut out = Vec::new();
  while let Some(delta) = stream.next().await {
    out.push(delta.map_err(anyhow::Error::msg)?);
  }
  Ok(out)
}

fn user(content: &str) -> ChatMessage {
  ChatMessage {
    role: Role::User,
    content: Some(content.to_string()),
    tool_call: None,
  }
}

fn build_provider(base_url: &str) -> anyhow::Result<ProviderEntry> {
  omw::provider::build(
    "openai",
    "openai",
    &json!({
      "base_url": format!("{base_url}/v1"),
      "api_key": "sk-test",
      "model": "gpt-test",
    }),
  )
}

#[tokio::test]
async fn content_deltas_stream_in_order() -> anyhow::Result<()> {
  let server = MockServer::start().await;
  let body = format!(
    "{}data: [DONE]\n\n",
    [
      sse_line(
        r#"{"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#
      ),
      sse_line(
        r#"{"choices":[{"delta":{"content":", world"},"finish_reason":null}]}"#
      ),
    ]
    .concat()
  );
  Mock::given(method("POST"))
    .and(path("/v1/chat/completions"))
    .respond_with(ResponseTemplate::new(200).set_body_string(body))
    .mount(&server)
    .await;

  let entry = build_provider(&server.uri())?;
  let deltas = chat_all(&entry, "gpt-test", vec![user("hi")]).await?;
  let content: Vec<Option<&str>> =
    deltas.iter().map(|d| d.content.as_deref()).collect();
  assert_eq!(content, vec![Some("Hello"), Some(", world")]);
  assert!(deltas.iter().all(|d| d.tool_call.is_none()));
  Ok(())
}

#[tokio::test]
async fn reassembles_tool_call_end_to_end() -> anyhow::Result<()> {
  let server = MockServer::start().await;
  let body = format!(
    "{}data: [DONE]\n\n",
    [
      sse_line(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"get_weather","arguments":""}}]},"finish_reason":null}]}"#),
      sse_line(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"city\":"}}]},"finish_reason":null}]}"#),
      sse_line(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"Paris\"}"}}]},"finish_reason":null}]}"#),
    ]
    .concat()
  );
  Mock::given(method("POST"))
    .and(path("/v1/chat/completions"))
    .respond_with(ResponseTemplate::new(200).set_body_string(body))
    .mount(&server)
    .await;

  let entry = build_provider(&server.uri())?;
  let deltas =
    chat_all(&entry, "gpt-test", vec![user("what's the weather?")]).await?;
  let tool_calls: Vec<Option<&ToolCall>> =
    deltas.iter().map(|d| d.tool_call.as_ref()).collect();
  // The `[DONE]` flush carries the fully reassembled arguments.
  let last = tool_calls
    .into_iter()
    .flatten()
    .last()
    .ok_or_else(|| anyhow::anyhow!("no tool call observed"))?;
  assert_eq!(last.id, "call_1");
  assert_eq!(last.name, "get_weather");
  assert_eq!(last.arguments, r#"{"city":"Paris"}"#);
  Ok(())
}

#[tokio::test]
async fn non_2xx_yields_an_error() -> anyhow::Result<()> {
  let server = MockServer::start().await;
  Mock::given(method("POST"))
    .and(path("/v1/chat/completions"))
    .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
    .mount(&server)
    .await;

  let entry = build_provider(&server.uri())?;
  let err = match entry
    .provider
    .chat("gpt-test", vec![user("hi")], Vec::new())
    .await
  {
    Ok(_) => anyhow::bail!("expected a non-2xx error"),
    Err(e) => e,
  };
  assert!(err.to_string().contains("status 500"), "{err}");
  Ok(())
}

#[tokio::test]
async fn sends_bearer_token_and_expected_payload() -> anyhow::Result<()> {
  let server = MockServer::start().await;
  Mock::given(method("POST"))
    .and(path("/v1/chat/completions"))
    .and(bearer_token("sk-test"))
    .and(body_partial_json(json!({
      "model": "gpt-test",
      "stream": true,
      "messages": [{ "role": "user", "content": "hi" }],
    })))
    .respond_with(ResponseTemplate::new(200).set_body_string(
      "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    ))
    .expect(1)
    .mount(&server)
    .await;

  let entry = build_provider(&server.uri())?;
  chat_all(&entry, "gpt-test", vec![user("hi")]).await?;
  server.verify().await;
  Ok(())
}

/// Serve a fragmented SSE body over real HTTP using chunked transfer-encoding,
/// delivering each chunk as its own network write with a flush + delay so
/// reqwest's `bytes_stream` yields them separately. This deterministically
/// exercises our partial-line buffering across HTTP chunks.
async fn serve_fragmented(chunks: Vec<String>) -> String {
  use tokio::io::{AsyncReadExt, AsyncWriteExt};
  use tokio::net::TcpListener;

  let listener = TcpListener::bind("127.0.0.1:0")
    .await
    .expect("bind streaming server");
  let addr = listener.local_addr().expect("streaming server local addr");
  let url = format!("http://{addr}/v1/chat/completions");

  tokio::spawn(async move {
    let (mut sock, _) = listener.accept().await.expect("accept connection");
    // Consume the request headers (up to the blank line).
    let mut header_buf = [0u8; 8192];
    let mut read = 0usize;
    loop {
      let n = sock
        .read(&mut header_buf[read..])
        .await
        .expect("read headers");
      read += n;
      if header_buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
        break;
      }
    }

    sock
      .write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
      )
      .await
      .expect("write head");
    for chunk in chunks {
      let size = format!("{:X}\r\n", chunk.len());
      sock.write_all(size.as_bytes()).await.expect("write size");
      sock.write_all(chunk.as_bytes()).await.expect("write chunk");
      sock.write_all(b"\r\n").await.expect("write crlf");
      sock.flush().await.expect("flush chunk");
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
    sock
      .write_all(b"0\r\n\r\n")
      .await
      .expect("write terminator");
  });

  url
}

#[tokio::test]
async fn fragmented_chunks_across_http_are_buffered() -> anyhow::Result<()> {
  // The string "Hello, world" is split mid-line inside a single SSE event so
  // that no complete line arrives in one network chunk.
  let url = serve_fragmented(vec![
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hel".to_string(),
    "lo, wo".to_string(),
    "rld\"},\"finish_reason\":null}]}".to_string(),
    "\n\n".to_string(),
    "data: [DONE]\n\n".to_string(),
  ])
  .await;

  let entry = omw::provider::build(
    "openai",
    "openai",
    &json!({ "base_url": url, "api_key": "sk-test" }),
  )?;
  let deltas = chat_all(&entry, "gpt-test", vec![user("hi")]).await?;
  let content: Vec<Option<String>> =
    deltas.iter().map(|d| d.content.clone()).collect();
  assert_eq!(content, vec![Some("Hello, world".to_string())]);
  Ok(())
}
