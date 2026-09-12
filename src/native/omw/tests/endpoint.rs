//! Heavy client test for the endpoint: drives `router` through the real
//! `async-openai` SDK (models list, non-streaming completion, SSE stream,
//! tool round-trip, unknown-model error, inbox shape) against a mock agent
//! that answers from the inbox.
#![cfg(feature = "endpoint-openai")]

use std::sync::Arc;
use std::time::Duration;

use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
  ChatCompletionRequestMessage, ChatCompletionRequestUserMessage,
  ChatCompletionTool, CreateChatCompletionRequestArgs, FunctionObjectArgs,
};
use futures_util::StreamExt;
use tokio::net::TcpListener;

use omw::endpoint::openai::{ServerState, router};
use omw::host::bus::MessageBus;
use omw::host::endpoint::EndpointRegistry;
use omw::host::events::Event;
use omw::provider::{ChatDelta, ChatMessage, Role, ToolCall};
use omw::tooling::Tool;

/// A mock agent: subscribes `model`, then answers every routed request with
/// `replies` deltas followed by a terminal `stop` delta.
struct MockAgent {
  registry: Arc<EndpointRegistry>,
  _task: tokio::task::JoinHandle<()>,
}

impl MockAgent {
  fn spawn(model: &str, replies: Vec<ChatDelta>) -> (Arc<MessageBus>, Self) {
    let bus = Arc::new(MessageBus::new());
    let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
    bus
      .endpoint_subscribe("alice", model.to_string())
      .expect("mock agent subscribes");
    let task_bus = Arc::clone(&bus);
    let task_registry = Arc::clone(&registry);
    let task = tokio::spawn(async move {
      loop {
        let envelope = match task_bus.try_recv("alice") {
          Ok(Some(envelope)) => envelope,
          // Yield to the scheduler; a fixed sleep here only adds reaction
          // latency to every SDK round-trip on CI.
          Ok(None) => {
            tokio::task::yield_now().await;
            continue;
          }
          Err(_) => return,
        };
        let Event::EndpointMessage(message) = envelope.event else {
          continue;
        };
        for delta in replies.clone() {
          if task_registry
            .push("alice", &message.session, delta)
            .is_err()
          {
            break;
          }
        }
        let _ = task_registry.push(
          "alice",
          &message.session,
          ChatDelta {
            content: None,
            tool_call: None,
            finish_reason: Some("stop".to_string()),
          },
        );
      }
    });
    (
      Arc::clone(&bus),
      Self {
        registry,
        _task: task,
      },
    )
  }
}

async fn serve(
  bus: Arc<MessageBus>,
  registry: Arc<EndpointRegistry>,
) -> anyhow::Result<String> {
  let listener = TcpListener::bind("127.0.0.1:0").await?;
  let base = format!("http://{}", listener.local_addr()?);
  tokio::spawn(async move {
    if let Err(error) =
      axum::serve(listener, router(ServerState::new(bus, registry))).await
    {
      tracing::error!(error = %error, "endpoint test server failed");
    }
  });
  Ok(base)
}

fn client_for(base: &str) -> Client<OpenAIConfig> {
  // async-openai appends `/chat/completions` etc. to the base, so point it at
  // our `/v1` prefix.
  let config = OpenAIConfig::new()
    .with_api_key("sk-test")
    .with_api_base(format!("{base}/v1"));
  Client::with_config(config)
}

fn user(content: &str) -> ChatCompletionRequestMessage {
  ChatCompletionRequestUserMessage::from(content).into()
}

#[tokio::test]
async fn models_list_through_sdk() -> anyhow::Result<()> {
  let (bus, agent) = MockAgent::spawn("gpt-4o", Vec::new());
  let base = serve(Arc::clone(&bus), Arc::clone(&agent.registry)).await?;
  let models = client_for(&base).models().list().await?;
  let mut ids: Vec<&str> = models.data.iter().map(|m| m.id.as_str()).collect();
  ids.sort_unstable();
  assert_eq!(ids, vec!["gpt-4o"]);
  Ok(())
}

#[tokio::test]
async fn non_streaming_completion_through_sdk() -> anyhow::Result<()> {
  let (bus, agent) = MockAgent::spawn(
    "gpt-4o",
    vec![ChatDelta {
      content: Some("Hello".to_string()),
      tool_call: None,
      finish_reason: None,
    }],
  );
  let base = serve(Arc::clone(&bus), Arc::clone(&agent.registry)).await?;
  let request = CreateChatCompletionRequestArgs::default()
    .model("gpt-4o")
    .messages([user("hi")])
    .build()?;
  let response = client_for(&base).chat().create(request).await?;
  let choice = response
    .choices
    .into_iter()
    .next()
    .ok_or_else(|| anyhow::anyhow!("expected at least one choice"))?;
  assert_eq!(choice.message.content.as_deref(), Some("Hello"));
  assert!(
    choice.finish_reason.is_some(),
    "completion should carry a finish reason"
  );
  Ok(())
}

#[tokio::test]
async fn streaming_completion_through_sdk() -> anyhow::Result<()> {
  let (bus, agent) = MockAgent::spawn(
    "gpt-4o",
    vec![
      ChatDelta {
        content: Some("Hel".to_string()),
        tool_call: None,
        finish_reason: None,
      },
      ChatDelta {
        content: Some("lo".to_string()),
        tool_call: None,
        finish_reason: None,
      },
    ],
  );
  let base = serve(Arc::clone(&bus), Arc::clone(&agent.registry)).await?;
  let request = CreateChatCompletionRequestArgs::default()
    .model("gpt-4o")
    .messages([user("hi")])
    .build()?;
  let mut stream = client_for(&base).chat().create_stream(request).await?;
  let mut content = String::new();
  let mut finished = false;
  while let Some(chunk) = stream.next().await {
    let chunk = chunk?;
    for choice in chunk.choices {
      if let Some(text) = choice.delta.content {
        content.push_str(&text);
      }
      if choice.finish_reason.is_some() {
        finished = true;
      }
    }
  }
  assert_eq!(content, "Hello");
  assert!(finished, "stream should end with a finish reason");
  Ok(())
}

#[tokio::test]
async fn tool_call_round_trip_through_sdk() -> anyhow::Result<()> {
  let (bus, agent) = MockAgent::spawn(
    "gpt-4o",
    vec![ChatDelta {
      content: None,
      tool_call: Some(ToolCall {
        id: "call_1".to_string(),
        name: "get_weather".to_string(),
        arguments: r#"{"location":"Boston"}"#.to_string(),
      }),
      finish_reason: None,
    }],
  );
  let base = serve(Arc::clone(&bus), Arc::clone(&agent.registry)).await?;
  let request = CreateChatCompletionRequestArgs::default()
    .model("gpt-4o")
    .messages([user("weather in Boston?")])
    .tools(ChatCompletionTool {
      function: FunctionObjectArgs::default()
        .name("get_weather")
        .description("Get the current weather in a given location")
        .parameters(serde_json::json!({
          "type": "object",
          "properties": {
            "location": { "type": "string" },
          },
          "required": ["location"],
        }))
        .build()?,
    })
    .build()?;
  let response = client_for(&base).chat().create(request).await?;
  let message = response
    .choices
    .into_iter()
    .next()
    .ok_or_else(|| anyhow::anyhow!("expected at least one choice"))?
    .message;
  let Some(tool_calls) = message.tool_calls else {
    return Err(anyhow::anyhow!("expected tool calls, got {message:?}"));
  };
  assert_eq!(tool_calls.len(), 1);
  let async_openai::types::chat::ChatCompletionMessageToolCalls::Function(call) =
    &tool_calls[0]
  else {
    return Err(anyhow::anyhow!(
      "expected a function tool call, got {:?}",
      tool_calls[0]
    ));
  };
  assert_eq!(call.id, "call_1");
  assert_eq!(call.function.name, "get_weather");
  assert_eq!(
    call.function.arguments,
    r#"{"location":"Boston"}"#.to_string()
  );
  Ok(())
}

#[tokio::test]
async fn unknown_model_errors_through_sdk() -> anyhow::Result<()> {
  let (bus, agent) = MockAgent::spawn("gpt-4o", Vec::new());
  let base = serve(Arc::clone(&bus), Arc::clone(&agent.registry)).await?;
  let request = CreateChatCompletionRequestArgs::default()
    .model("nope")
    .messages([user("hi")])
    .build()?;
  let error = client_for(&base)
    .chat()
    .create(request)
    .await
    .expect_err("unknown model should error");
  assert!(
    error.to_string().contains("nope"),
    "error should name the model, got {error:?}"
  );
  Ok(())
}

#[tokio::test]
async fn inbound_messages_and_tools_land_in_agent_inbox() -> anyhow::Result<()>
{
  // Serve without the mock-agent loop so this test owns the inbox: subscribe,
  // POST through the SDK, then inspect the raw event.
  let bus = Arc::new(MessageBus::new());
  let registry = Arc::new(EndpointRegistry::new(Arc::clone(&bus)));
  bus
    .endpoint_subscribe("alice", "gpt-4o".to_string())
    .map_err(|e| anyhow::anyhow!(e))?;
  let base = serve(Arc::clone(&bus), Arc::clone(&registry)).await?;
  let request = CreateChatCompletionRequestArgs::default()
    .model("gpt-4o")
    .messages([user("hi")])
    .tools(ChatCompletionTool {
      function: FunctionObjectArgs::default()
        .name("get_weather")
        .parameters(serde_json::json!({ "type": "object" }))
        .build()?,
    })
    .build()?;
  let reader = tokio::spawn({
    let client = client_for(&base);
    async move { client.chat().create(request).await }
  });
  let mut session = None;
  let mut messages = Vec::new();
  let mut tools = Vec::new();
  // Generous budget: server POST -> route on a loaded CI VM can take
  // seconds.
  for _ in 0..1000 {
    if let Some(envelope) = bus.try_recv("alice")? {
      if let Event::EndpointMessage(message) = envelope.event {
        session = Some(message.session.clone());
        messages = message.messages;
        tools = message.tools;
        break;
      }
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
  }
  let Some(session) = session else {
    return Err(anyhow::anyhow!("no endpoint-message arrived"));
  };
  assert_eq!(
    messages,
    vec![ChatMessage {
      role: Role::User,
      content: Some("hi".to_string()),
      tool_call: None,
    }]
  );
  assert_eq!(
    tools,
    vec![Tool {
      name: "get_weather".to_string(),
      description: None,
      input_schema: serde_json::json!({ "type": "object" }),
    }]
  );
  // Answer so the POST completes, then assert the terminal end event fires
  // exactly once.
  registry
    .push(
      "alice",
      &session,
      ChatDelta {
        content: None,
        tool_call: None,
        finish_reason: Some("stop".to_string()),
      },
    )
    .map_err(|e| anyhow::anyhow!(e))?;
  reader
    .await
    .map_err(|e| anyhow::anyhow!("reader failed: {e}"))?
    .map_err(|e| anyhow::anyhow!("sdk request failed: {e}"))?;
  let end = bus
    .try_recv("alice")?
    .ok_or_else(|| anyhow::anyhow!("expected an endpoint-session-end"))?;
  let Event::EndpointSessionEnd(end) = end.event else {
    return Err(anyhow::anyhow!(
      "expected an endpoint-session-end, got {end:?}"
    ));
  };
  assert_eq!(end.session, session);
  assert!(end.error.is_none());
  assert!(
    bus.try_recv("alice")?.is_none(),
    "exactly one session end should fire"
  );
  Ok(())
}
