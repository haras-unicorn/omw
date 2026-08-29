//! Integration test for the OpenAI-family provider against a *real* inference
//! endpoint: the official `ghcr.io/ggml-org/llama.cpp:server` container serving
//! the OpenAI-compatible `/v1/chat/completions` API with the GGUF model that the
//! flake already fetches.
//!
//! This exercises real SSE streaming and real token generation through
//! `OpenAIProvider`, rather than a mocked HTTP server.
//!
//! Opt-in via `OMW_TEST_OPENAI_LLAMACPP`. Some tests are also ignored by default
//! because of flakiness of LLM models.

use std::time::Duration;

use anyhow::Context as _;
use futures_util::StreamExt;
use serde_json::json;
use testcontainers::core::{IntoContainerPort, Mount};
use testcontainers::{
  ContainerAsync, GenericImage, ImageExt, runners::AsyncRunner,
};

use omw::provider::{ChatMessage, Role};

fn enabled() -> bool {
  std::env::var_os("OMW_TEST_OPENAI_LLAMACPP").is_some_and(|value| value != "0")
}

/// Poll the llama.cpp `/health` endpoint until the server reports ready, or
/// give up.
async fn wait_healthy(
  client: &reqwest::Client,
  health_url: &str,
) -> anyhow::Result<()> {
  for _ in 0..300 {
    let ok = client
      .get(health_url)
      .send()
      .await
      .map(|r| r.status().is_success())
      .unwrap_or(false);
    if ok {
      return Ok(());
    }
    tokio::time::sleep(Duration::from_millis(1000)).await;
  }
  anyhow::bail!("llama.cpp server did not become healthy at {health_url}");
}

/// Start the llama.cpp server container with the fetched GGUF bind-mounted in,
/// mapped to a host port, and return (the container, the server base URL).
async fn start_llamacpp()
-> anyhow::Result<(ContainerAsync<GenericImage>, String)> {
  let gguf = std::env::var("OMW_TEST_OPENAI_LLAMACPP_GGUF").context(
    "OMW_TEST_OPENAI_LLAMACPP_GGUF must point at the GGUF model file",
  )?;
  let model_file = std::env::var("OMW_TEST_OPENAI_LLAMACPP_MODEL")
    .unwrap_or_else(|_| "model.gguf".into());
  let container_target = format!("/models/{model_file}");

  let image = GenericImage::new("ghcr.io/ggml-org/llama.cpp", "server")
    .with_exposed_port(8080.tcp())
    .with_mount(Mount::bind_mount(&gguf, &container_target))
    .with_startup_timeout(Duration::from_secs(600))
    .with_cmd([
      "-m",
      container_target.as_str(),
      "--host",
      "0.0.0.0",
      "--port",
      "8080",
      "-c",
      "1024",
      "--n-gpu-layers",
      "0",
    ]);

  let container = image.start().await.map_err(anyhow::Error::msg)?;
  let host = container.get_host().await?.to_string();
  let port = container.get_host_port_ipv4(8080).await?;
  let base = format!("http://{host}:{port}");

  let client = reqwest::Client::new();
  wait_healthy(&client, &format!("{base}/health")).await?;
  Ok((container, base))
}

#[tokio::test]
#[ignore = "it is flaky but it is good to have it"]
async fn llamacpp_streams_real_inference() -> anyhow::Result<()> {
  if !enabled() {
    eprintln!("skipping: OMW_TEST_OPENAI_LLAMACPP not set");
    return Ok(());
  }

  let (_container, base) = start_llamacpp().await?;

  let entry = omw::provider::build(
    "local",
    "openai",
    &json!({
      "base_url": format!("{base}/v1"),
      "api_key": "none",
      "model": "omw-test",
    }),
  )?;
  let system_msg = ChatMessage {
    role: Role::System,
    content: Some(
      "You are a helpful assistant that does what the user says.".to_string(),
    ),
    tool_call: None,
  };
  let user_msg = ChatMessage {
    role: Role::User,
    content: Some("Reply with exactly the single word: 'pong'.".to_string()),
    tool_call: None,
  };
  let mut stream = entry
    .provider
    .chat("omw-test", vec![system_msg, user_msg], Vec::new())
    .await?;

  let mut content = String::new();
  while let Some(delta) = stream.next().await {
    let delta = delta.map_err(anyhow::Error::msg)?;
    if let Some(text) = delta.content {
      content.push_str(&text);
    }
  }

  eprintln!("llama.cpp replied: {content}");
  assert!(!content.trim().is_empty(), "llama.cpp returned no content");
  assert!(
    content.trim().contains("pong"),
    "llama.cpp did not return 'pong'"
  );
  Ok(())
}
