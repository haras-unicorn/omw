//! Integration tests for the OpenAI-family provider against a hermetic
//! in-process HTTP server (wiremock) that speaks the SSE chat wire protocol.
//!
//! These pin our parsing/encoding over real HTTP: content deltas, tool-call
//! reassembly across fragmented chunks, error surfacing, and the outgoing
//! request (bearer token, model, streaming).
#![cfg(feature = "provider-openai")]

use std::time::Duration;

use futures_util::StreamExt;
use omw::provider::{ChatMessage, ProviderEntry, ToolCall};
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
  let mut stream = entry
    .inner()
    .chat_stream(model, messages, Vec::new(), None)
    .await?;
  let mut out = Vec::new();
  while let Some(delta) = stream.next().await {
    out.push(delta.map_err(anyhow::Error::msg)?);
  }
  Ok(out)
}

fn user(content: &str) -> ChatMessage {
  ChatMessage {
    role: "user".to_string(),
    content: Some(content.to_string()),
    reasoning: None,
    tool_calls: Vec::new(),
    tool_call_id: None,
  }
}

fn build_provider(base_url: &str) -> anyhow::Result<ProviderEntry> {
  omw::provider::Registry::default().build(
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
  assert!(deltas.iter().all(|d| d.tool_calls.is_empty()));
  Ok(())
}

#[tokio::test]
async fn surfaces_tool_call_fragments_and_reassembles_in_blocking_chat()
-> anyhow::Result<()> {
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
  // Streaming surfaces the fragments verbatim, tagged with their index.
  let fragments: Vec<&omw::provider::ToolCallDelta> =
    deltas.iter().flat_map(|d| d.tool_calls.iter()).collect();
  assert_eq!(fragments.len(), 3);
  assert!(fragments.iter().all(|f| f.index == 0));
  assert_eq!(fragments[0].id.as_deref(), Some("call_1"));
  assert_eq!(fragments[0].name.as_deref(), Some("get_weather"));
  assert_eq!(fragments[2].arguments.as_deref(), Some(r#""Paris"}"#));

  // The blocking `chat` path reassembles them into one complete call.
  let result = entry
    .inner()
    .chat(
      "gpt-test",
      vec![user("what's the weather?")],
      Vec::new(),
      None,
    )
    .await?;
  assert_eq!(result.tool_calls.len(), 1);
  assert_eq!(result.tool_calls[0].id, "call_1");
  assert_eq!(result.tool_calls[0].name, "get_weather");
  assert_eq!(result.tool_calls[0].arguments, r#"{"city":"Paris"}"#);
  Ok(())
}

#[tokio::test]
async fn sends_parallel_tool_calls_and_tool_call_id() -> anyhow::Result<()> {
  let server = MockServer::start().await;
  Mock::given(method("POST"))
    .and(path("/v1/chat/completions"))
    .and(body_partial_json(json!({
      "messages": [
        { "role": "assistant", "tool_calls": [
          { "id": "call_1", "type": "function", "function": { "name": "a", "arguments": "{}" } },
          { "id": "call_2", "type": "function", "function": { "name": "b", "arguments": "{}" } },
        ]},
        { "role": "tool", "content": "ok", "tool_call_id": "call_1" },
      ],
    })))
    .respond_with(ResponseTemplate::new(200).set_body_string(
      "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    ))
    .expect(1)
    .mount(&server)
    .await;

  let entry = build_provider(&server.uri())?;
  let messages = vec![
    ChatMessage {
      role: "assistant".to_string(),
      content: None,
      reasoning: None,
      tool_calls: vec![
        ToolCall {
          id: "call_1".to_string(),
          name: "a".to_string(),
          arguments: "{}".to_string(),
        },
        ToolCall {
          id: "call_2".to_string(),
          name: "b".to_string(),
          arguments: "{}".to_string(),
        },
      ],
      tool_call_id: None,
    },
    ChatMessage {
      role: "tool".to_string(),
      content: Some("ok".to_string()),
      reasoning: None,
      tool_calls: Vec::new(),
      tool_call_id: Some("call_1".to_string()),
    },
  ];
  chat_all(&entry, "gpt-test", messages).await?;
  server.verify().await;
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
    .inner()
    .chat("gpt-test", vec![user("hi")], Vec::new(), None)
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

#[tokio::test]
async fn merges_config_defaults_call_params_and_pins_mandatory_fields()
-> anyhow::Result<()> {
  let server = MockServer::start().await;
  // Call params override config defaults; `model`/`stream` cannot be broken.
  Mock::given(method("POST"))
    .and(path("/v1/chat/completions"))
    .and(bearer_token("sk-test"))
    .and(body_partial_json(json!({
      "model": "gpt-test",
      "stream": true,
      "messages": [{ "role": "user", "content": "hi" }],
      "temperature": 0.2,
      "max_tokens": 32,
      "reasoning_effort": "high",
      "extra_flag": true,
    })))
    .respond_with(ResponseTemplate::new(200).set_body_string(
      "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    ))
    .expect(1)
    .mount(&server)
    .await;

  let entry = omw::provider::Registry::default().build(
    "openai",
    "openai",
    &json!({
      "base_url": format!("{}/v1", server.uri()),
      "api_key": "sk-test",
      "params": { "temperature": 0.9, "extra_flag": true },
    }),
  )?;
  let _stream = entry
    .inner()
    .chat_stream(
      "gpt-test",
      vec![user("hi")],
      Vec::new(),
      Some(json!({
        "temperature": 0.2,
        "max_tokens": 32,
        "reasoning_effort": "high",
        "model": "hacked",
        "stream": false,
      })),
    )
    .await?;
  server.verify().await;
  Ok(())
}

#[tokio::test]
async fn list_models_returns_the_endpoint_catalog() -> anyhow::Result<()> {
  let server = MockServer::start().await;
  Mock::given(method("GET"))
    .and(path("/v1/models"))
    .and(bearer_token("sk-test"))
    .respond_with(ResponseTemplate::new(200).set_body_json(json!({
      "data": [{ "id": "gpt-a" }, { "id": "gpt-b" }],
    })))
    .mount(&server)
    .await;

  let entry = build_provider(&server.uri())?;
  let models = entry.inner().list_models().await?;
  assert_eq!(models, vec!["gpt-a", "gpt-b"]);
  Ok(())
}

#[tokio::test]
async fn list_models_errors_when_the_endpoint_fails() -> anyhow::Result<()> {
  let server = MockServer::start().await;
  Mock::given(method("GET"))
    .and(path("/v1/models"))
    .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
    .mount(&server)
    .await;

  let entry = build_provider(&server.uri())?;
  let error = entry
    .inner()
    .list_models()
    .await
    .expect_err("a failed model request should error");
  assert!(error.to_string().contains("status 500"), "{error}");
  Ok(())
}

#[tokio::test]
async fn list_models_falls_back_to_the_configured_model_when_empty()
-> anyhow::Result<()> {
  let server = MockServer::start().await;
  Mock::given(method("GET"))
    .and(path("/v1/models"))
    .respond_with(
      ResponseTemplate::new(200).set_body_json(json!({ "data": [] })),
    )
    .mount(&server)
    .await;

  let entry = build_provider(&server.uri())?;
  let models = entry.inner().list_models().await?;
  assert_eq!(models, vec!["gpt-test"]);
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

  let entry = omw::provider::Registry::default().build(
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
