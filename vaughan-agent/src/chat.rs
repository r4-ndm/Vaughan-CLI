//! Assist-mode chat turn: stream LLM replies, run tools, surface proposals.

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{mpsc, watch};

use crate::client::{LlmClient, StreamEvent};
use crate::degen::policy::PolicyProposal;
use crate::error::AgentError;
use crate::proposal::TxProposal;
use crate::tools::{ToolContext, ToolRegistry};
use crate::types::{ChatMessage, ToolDefinition};

/// Max LLM↔tool rounds for a single user turn (prevents runaway loops).
pub const MAX_TOOL_ROUNDS: usize = 8;

/// UI-facing events emitted while a chat turn runs.
#[derive(Debug, Clone)]
pub enum ChatUiEvent {
    /// Status line (e.g. "thinking…", "calling inspect_contract").
    Status(String),
    /// Incremental assistant text.
    Delta(String),
    /// Tool about to run.
    ToolCall { name: String, args: String },
    /// Tool finished.
    ToolResult { name: String, result: String },
    /// Propose-only write tool produced a human-approval card.
    Proposal(Box<TxProposal>),
    /// Degen policy change awaiting human `[a]` / `[d]`.
    PolicyProposal(Box<PolicyProposal>),
    /// Turn completed; `history` is the full conversation to persist.
    Finished { history: Vec<ChatMessage> },
    /// Turn aborted by Esc / kill switch.
    Cancelled { history: Vec<ChatMessage> },
    /// Fatal error for the turn.
    Error {
        message: String,
        history: Vec<ChatMessage>,
    },
}

fn cancelled(cancel: &watch::Receiver<bool>) -> bool {
    *cancel.borrow()
}

/// Propose / autonomous write tools that must be preceded by a sensory tool.
pub fn is_propose_tool(name: &str) -> bool {
    name.starts_with("propose_")
}

/// Degen autonomous writes (same sensory unlock as propose_*).
pub fn is_degen_execute_tool(name: &str) -> bool {
    name.starts_with("execute_degen_")
}

/// Read-only / simulation tools that unlock propose_* / execute_degen_* in the same turn.
pub fn is_sensory_tool(name: &str) -> bool {
    matches!(
        name,
        "inspect_contract" | "get_balance" | "get_dex_reserves" | "search_pairs" | "simulate_call"
    )
}

/// Run one Assist-mode user turn with streaming and tool execution.
///
/// Appends the user message and subsequent assistant/tool messages onto
/// `history`. Emits [`ChatUiEvent`]s on `ui_tx`. Set `*cancel` to `true` to abort.
///
/// **Guard:** `propose_*` / `execute_degen_*` tools are rejected unless a sensory
/// tool already succeeded earlier in this turn (eval / anti-hallucination).
pub async fn run_assist_turn(
    history: &mut Vec<ChatMessage>,
    client: Arc<dyn LlmClient>,
    registry: &ToolRegistry,
    context: &ToolContext,
    user_text: impl Into<String>,
    ui_tx: mpsc::UnboundedSender<ChatUiEvent>,
    cancel: watch::Receiver<bool>,
) -> Result<(), AgentError> {
    let user_text = user_text.into();
    history.push(ChatMessage::user(user_text));

    let tools: Vec<ToolDefinition> = registry.definitions();
    let _ = ui_tx.send(ChatUiEvent::Status(format!(
        "thinking ({})…",
        client.name()
    )));

    let mut saw_sensory = false;

    for _round in 0..MAX_TOOL_ROUNDS {
        if cancelled(&cancel) {
            let _ = ui_tx.send(ChatUiEvent::Cancelled {
                history: history.clone(),
            });
            return Err(AgentError::ExecutionAborted);
        }

        let (stream_tx, mut stream_rx) = mpsc::channel::<StreamEvent>(64);
        let client_clone = Arc::clone(&client);
        let messages = history.clone();
        let tools_clone = tools.clone();
        let cancel_clone = cancel.clone();

        let join = tokio::spawn(async move {
            client_clone
                .stream(&messages, &tools_clone, stream_tx, cancel_clone)
                .await
        });

        while let Some(event) = stream_rx.recv().await {
            match event {
                StreamEvent::Delta(delta) => {
                    let _ = ui_tx.send(ChatUiEvent::Delta(delta));
                }
            }
        }

        let assistant = match join.await {
            Ok(Ok(msg)) => msg,
            Ok(Err(AgentError::ExecutionAborted)) => {
                let _ = ui_tx.send(ChatUiEvent::Cancelled {
                    history: history.clone(),
                });
                return Err(AgentError::ExecutionAborted);
            }
            Ok(Err(e)) => {
                let _ = ui_tx.send(ChatUiEvent::Error {
                    message: e.to_string(),
                    history: history.clone(),
                });
                return Err(e);
            }
            Err(e) => {
                let err = AgentError::ProviderError(format!("LLM task failed: {e}"));
                let _ = ui_tx.send(ChatUiEvent::Error {
                    message: err.to_string(),
                    history: history.clone(),
                });
                return Err(err);
            }
        };

        if cancelled(&cancel) {
            let _ = ui_tx.send(ChatUiEvent::Cancelled {
                history: history.clone(),
            });
            return Err(AgentError::ExecutionAborted);
        }

        let tool_calls = assistant.tool_calls.clone().unwrap_or_default();
        history.push(assistant);

        if tool_calls.is_empty() {
            let _ = ui_tx.send(ChatUiEvent::Status(String::new()));
            let _ = ui_tx.send(ChatUiEvent::Finished {
                history: history.clone(),
            });
            return Ok(());
        }

        for call in tool_calls {
            if cancelled(&cancel) {
                let _ = ui_tx.send(ChatUiEvent::Cancelled {
                    history: history.clone(),
                });
                return Err(AgentError::ExecutionAborted);
            }

            let args_preview = call.arguments.to_string();
            let _ = ui_tx.send(ChatUiEvent::Status(format!("calling {}…", call.name)));
            let _ = ui_tx.send(ChatUiEvent::ToolCall {
                name: call.name.clone(),
                args: args_preview,
            });

            if (is_propose_tool(&call.name) || is_degen_execute_tool(&call.name))
                && call.name != "propose_policy"
                && !saw_sensory
            {
                let err_text = format!(
                    "refused: call a sensory tool (inspect_contract, get_balance, \
                     get_dex_reserves, search_pairs, or simulate_call) before {}",
                    call.name
                );
                let _ = ui_tx.send(ChatUiEvent::ToolResult {
                    name: call.name.clone(),
                    result: format!("error: {err_text}"),
                });
                history.push(ChatMessage::tool_response(
                    call.id,
                    format!(r#"{{"error":{}}}"#, Value::String(err_text)),
                ));
                continue;
            }

            match registry
                .execute(&call.name, call.arguments.clone(), context)
                .await
            {
                Ok(value) => {
                    if is_sensory_tool(&call.name) {
                        saw_sensory = true;
                    }
                    maybe_emit_proposal(&value, &ui_tx);
                    let result_text = value.to_string();
                    let _ = ui_tx.send(ChatUiEvent::ToolResult {
                        name: call.name.clone(),
                        result: summarize_tool_json(&call.name, &value),
                    });
                    // Keep history compact so the next LLM round isn't drowned in JSON.
                    history.push(ChatMessage::tool_response(
                        call.id,
                        truncate(&result_text, MAX_HISTORY_TOOL_CHARS),
                    ));
                }
                Err(e) => {
                    let err_text = e.to_string();
                    let _ = ui_tx.send(ChatUiEvent::ToolResult {
                        name: call.name.clone(),
                        result: format!("error: {err_text}"),
                    });
                    history.push(ChatMessage::tool_response(
                        call.id,
                        format!(r#"{{"error":{}}}"#, Value::String(err_text)),
                    ));
                }
            }
        }

        let _ = ui_tx.send(ChatUiEvent::Status(format!(
            "thinking ({})…",
            client.name()
        )));
    }

    let err = AgentError::ProviderError(format!(
        "tool loop exceeded {MAX_TOOL_ROUNDS} rounds without a final reply — \
         try a narrower question, or ask for a short summary of the last tool result"
    ));
    let _ = ui_tx.send(ChatUiEvent::Error {
        message: err.to_string(),
        history: history.clone(),
    });
    Err(err)
}

fn maybe_emit_proposal(value: &Value, ui_tx: &mpsc::UnboundedSender<ChatUiEvent>) {
    if let Ok(prop) = serde_json::from_value::<TxProposal>(value.clone()) {
        let _ = ui_tx.send(ChatUiEvent::Proposal(Box::new(prop)));
        return;
    }
    if let Ok(prop) = serde_json::from_value::<PolicyProposal>(value.clone()) {
        let _ = ui_tx.send(ChatUiEvent::PolicyProposal(Box::new(prop)));
    }
}

/// Soft cap on tool JSON stored in the LLM conversation (UI uses a short summary).
const MAX_HISTORY_TOOL_CHARS: usize = 3_000;

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

/// Short human-readable line for the TUI (never dump huge JSON blobs).
pub fn summarize_tool_json(name: &str, value: &Value) -> String {
    if let Some(err) = value.get("error").and_then(|v| v.as_str()) {
        return format!("error: {err}");
    }
    match name {
        "inspect_contract" => {
            let addr = value.get("address").and_then(|v| v.as_str()).unwrap_or("?");
            let fp = value.get("fingerprint");
            let kind = fp
                .and_then(|f| f.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown");
            let details = fp.and_then(|f| f.get("details"));
            let mut bits = vec![format!("{kind} @ {addr}")];
            if let Some(sym) = details
                .and_then(|d| d.get("symbol"))
                .and_then(|v| v.as_str())
            {
                bits.push(format!("symbol={}", collapse_ws(sym)));
            }
            if let Some(nm) = details.and_then(|d| d.get("name")).and_then(|v| v.as_str()) {
                bits.push(format!("name={}", collapse_ws(nm)));
            }
            let sel_n = value
                .get("candidate_selector_count")
                .and_then(|v| v.as_u64())
                .or_else(|| {
                    value
                        .get("candidate_selectors")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len() as u64)
                })
                .unwrap_or(0);
            bits.push(format!("{sel_n} selectors"));
            if value
                .get("candidate_selectors_truncated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                bits.push("list truncated".into());
            }
            bits.join(" · ")
        }
        "get_balance" => {
            if let Some(wei) = value.get("balance_wei").and_then(|v| v.as_str()) {
                return format!("native balance_wei={wei}");
            }
            if let Some(raw) = value.get("balance_raw").and_then(|v| v.as_str()) {
                let token = value.get("token").and_then(|v| v.as_str()).unwrap_or("?");
                return format!("token={token} balance_raw={raw}");
            }
            truncate(&value.to_string(), 240)
        }
        _ => truncate(&value.to_string(), 280),
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn propose_and_sensory_classification() {
        assert!(is_propose_tool("propose_transfer"));
        assert!(is_propose_tool("propose_swap"));
        assert!(!is_propose_tool("get_balance"));
        assert!(is_degen_execute_tool("execute_degen_swap"));
        assert!(!is_degen_execute_tool("propose_swap"));
        assert!(is_sensory_tool("inspect_contract"));
        assert!(is_sensory_tool("simulate_call"));
        assert!(!is_sensory_tool("propose_transfer"));
    }

    #[test]
    fn summarize_inspect_is_one_line() {
        let v = serde_json::json!({
            "address": "0xabc",
            "fingerprint": {
                "type": "Erc20",
                "details": {
                    "name": "AC\n  eXchange\nToken",
                    "symbol": "ACXT",
                    "decimals": null
                }
            },
            "candidate_selector_count": 120,
            "candidate_selectors": ["0x70a08231"],
            "candidate_selectors_truncated": true
        });
        let s = summarize_tool_json("inspect_contract", &v);
        assert!(s.contains("Erc20"));
        assert!(s.contains("ACXT"));
        assert!(s.contains("AC eXchange Token"));
        assert!(s.contains("120 selectors"));
        assert!(s.contains("list truncated"));
        assert!(!s.contains("0x70a08231"));
    }
}
