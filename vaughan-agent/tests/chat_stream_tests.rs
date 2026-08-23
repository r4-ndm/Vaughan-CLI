//! Unit tests for Assist-mode streaming chat turns.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};
use vaughan_agent::client::{LlmClient, StreamEvent};
use vaughan_agent::error::AgentError;
use vaughan_agent::tools::{ToolContext, ToolRegistry};
use vaughan_agent::types::{ChatMessage, Role, ToolCall, ToolDefinition};
use vaughan_agent::{assist_system_prompt, run_assist_turn, ChatUiEvent};

struct ScriptedClient {
    /// Pre-canned assistant messages returned in order (one per stream call).
    replies: std::sync::Mutex<Vec<ChatMessage>>,
}

impl ScriptedClient {
    fn new(replies: Vec<ChatMessage>) -> Self {
        Self {
            replies: std::sync::Mutex::new(replies),
        }
    }
}

#[async_trait]
impl LlmClient for ScriptedClient {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn complete(
        &self,
        _messages: &[ChatMessage],
        _tools: &[ToolDefinition],
    ) -> Result<ChatMessage, AgentError> {
        let mut replies = self.replies.lock().unwrap();
        replies
            .pop()
            .ok_or_else(|| AgentError::ProviderError("no scripted replies left".into()))
    }

    async fn stream(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        event_tx: mpsc::Sender<StreamEvent>,
        cancel: watch::Receiver<bool>,
    ) -> Result<ChatMessage, AgentError> {
        if *cancel.borrow() {
            return Err(AgentError::ExecutionAborted);
        }
        // Pop from front: store reversed, so we pop from end of vec as queue.
        let message = {
            let mut replies = self.replies.lock().unwrap();
            if replies.is_empty() {
                return Err(AgentError::ProviderError("no scripted replies left".into()));
            }
            replies.remove(0)
        };
        let _ = (messages, tools);
        // Emit content character-by-character-ish (chunked) to exercise deltas.
        for chunk in message.content.as_bytes().chunks(8) {
            if *cancel.borrow() {
                return Err(AgentError::ExecutionAborted);
            }
            let delta = String::from_utf8_lossy(chunk).to_string();
            let _ = event_tx.send(StreamEvent::Delta(delta)).await;
        }
        Ok(message)
    }
}

#[tokio::test]
async fn assist_turn_streams_plain_reply_without_tools() {
    let client = Arc::new(ScriptedClient::new(vec![ChatMessage::assistant(
        "Hello from Vaughan",
    )]));
    let registry = ToolRegistry::new();
    let context = ToolContext {
        rpc_url: "http://127.0.0.1:8545".into(),
        chain_id: 943,
        active_address: None,
    };
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let mut history = vec![assist_system_prompt()];

    run_assist_turn(
        &mut history,
        client,
        &registry,
        &context,
        "hi",
        ui_tx,
        cancel_rx,
    )
    .await
    .unwrap();

    let mut deltas = String::new();
    let mut finished = false;
    while let Ok(ev) = ui_rx.try_recv() {
        match ev {
            ChatUiEvent::Delta(d) => deltas.push_str(&d),
            ChatUiEvent::Finished { .. } => finished = true,
            _ => {}
        }
    }
    assert!(finished);
    assert_eq!(deltas, "Hello from Vaughan");
    assert_eq!(history.last().unwrap().role, Role::Assistant);
}

#[tokio::test]
async fn assist_turn_runs_tool_then_final_reply() {
    let client = Arc::new(ScriptedClient::new(vec![
        ChatMessage::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "1".into(),
                name: "missing_tool".into(),
                arguments: serde_json::json!({}),
                thought_signature: None,
            }],
        ),
        ChatMessage::assistant("Tool failed as expected"),
    ]));
    let registry = ToolRegistry::new();
    let context = ToolContext {
        rpc_url: "http://127.0.0.1:8545".into(),
        chain_id: 943,
        active_address: None,
    };
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let mut history = vec![assist_system_prompt()];

    run_assist_turn(
        &mut history,
        client,
        &registry,
        &context,
        "do the thing",
        ui_tx,
        cancel_rx,
    )
    .await
    .unwrap();

    let mut saw_tool = false;
    let mut final_text = String::new();
    while let Ok(ev) = ui_rx.try_recv() {
        match ev {
            ChatUiEvent::ToolCall { name, .. } => {
                assert_eq!(name, "missing_tool");
                saw_tool = true;
            }
            ChatUiEvent::Delta(d) => final_text.push_str(&d),
            _ => {}
        }
    }
    assert!(saw_tool);
    assert_eq!(final_text, "Tool failed as expected");
}

#[tokio::test]
async fn assist_turn_honours_cancel_before_stream() {
    let client = Arc::new(ScriptedClient::new(vec![ChatMessage::assistant("Nope")]));
    let registry = ToolRegistry::new();
    let context = ToolContext {
        rpc_url: "http://127.0.0.1:8545".into(),
        chain_id: 943,
        active_address: None,
    };
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = watch::channel(true);
    let mut history = vec![assist_system_prompt()];

    let err = run_assist_turn(
        &mut history,
        client,
        &registry,
        &context,
        "hi",
        ui_tx,
        cancel_rx,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AgentError::ExecutionAborted));
    let _ = cancel_tx;

    let mut cancelled = false;
    while let Ok(ev) = ui_rx.try_recv() {
        if matches!(ev, ChatUiEvent::Cancelled { .. }) {
            cancelled = true;
        }
    }
    assert!(cancelled);
}

#[tokio::test]
async fn assist_turn_refuses_propose_without_prior_sensory_tool() {
    let client = Arc::new(ScriptedClient::new(vec![
        ChatMessage::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "1".into(),
                name: "propose_transfer".into(),
                arguments: serde_json::json!({
                    "to": "0x0000000000000000000000000000000000000001",
                    "value_wei": "1"
                }),
                thought_signature: None,
            }],
        ),
        ChatMessage::assistant("I will inspect first next time."),
    ]));
    let registry = ToolRegistry::new();
    let context = ToolContext {
        rpc_url: "http://127.0.0.1:8545".into(),
        chain_id: 943,
        active_address: None,
    };
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let mut history = vec![assist_system_prompt()];

    run_assist_turn(
        &mut history,
        client,
        &registry,
        &context,
        "send everything",
        ui_tx,
        cancel_rx,
    )
    .await
    .unwrap();

    let mut refused = false;
    while let Ok(ev) = ui_rx.try_recv() {
        if let ChatUiEvent::ToolResult { name, result } = ev {
            assert_eq!(name, "propose_transfer");
            assert!(
                result.contains("refused"),
                "expected refusal, got: {result}"
            );
            refused = true;
        }
    }
    assert!(refused, "propose_transfer must be refused without sensory");
}
