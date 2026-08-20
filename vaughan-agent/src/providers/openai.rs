//! OpenAI-compatible LLM client (works with OpenAI, DeepSeek, LocalAI, vLLM, Ollama).

use std::collections::HashMap;

use async_trait::async_trait;
use futures_util::StreamExt;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};

use crate::client::{LlmClient, StreamEvent};
use crate::error::AgentError;
use crate::types::{ChatMessage, ModelConfig, Role, ToolCall, ToolDefinition};

pub struct OpenAiClient {
    config: ModelConfig,
    client: reqwest::Client,
}

impl OpenAiClient {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    fn build_messages<'a>(messages: &'a [ChatMessage]) -> Vec<OpenAiMessage<'a>> {
        messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "tool",
                };
                let tool_calls = m.tool_calls.as_ref().map(|calls| {
                    calls
                        .iter()
                        .map(|c| OpenAiToolCall {
                            id: c.id.clone(),
                            call_type: "function".to_string(),
                            function: OpenAiFunctionCall {
                                name: c.name.clone(),
                                arguments: c.arguments.to_string(),
                            },
                        })
                        .collect()
                });
                OpenAiMessage {
                    role,
                    content: &m.content,
                    tool_calls,
                    tool_call_id: m.tool_call_id.as_deref(),
                }
            })
            .collect()
    }

    fn build_tools<'a>(tools: &'a [ToolDefinition]) -> Vec<OpenAiTool<'a>> {
        tools
            .iter()
            .map(|t| OpenAiTool {
                tool_type: "function",
                function: OpenAiFunctionDef {
                    name: &t.name,
                    description: &t.description,
                    parameters: &t.parameters,
                },
            })
            .collect()
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.config.endpoint_url.trim_end_matches('/')
        )
    }
}

#[derive(Serialize)]
struct OpenAiChatRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAiMessage<'a>>,
    temperature: f32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiTool<'a>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
}

#[derive(Serialize)]
struct OpenAiMessage<'a> {
    role: &'a str,
    content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OpenAiFunctionCall,
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAiFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct OpenAiTool<'a> {
    #[serde(rename = "type")]
    tool_type: &'static str,
    function: OpenAiFunctionDef<'a>,
}

#[derive(Serialize)]
struct OpenAiFunctionDef<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a Value,
}

#[derive(Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
}

#[derive(Deserialize)]
struct OpenAiResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Deserialize)]
struct OpenAiStreamChunk {
    choices: Vec<OpenAiStreamChoice>,
}

#[derive(Deserialize)]
struct OpenAiStreamChoice {
    delta: OpenAiStreamDelta,
}

#[derive(Deserialize)]
struct OpenAiStreamDelta {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiStreamToolCall>>,
}

#[derive(Deserialize)]
struct OpenAiStreamToolCall {
    index: usize,
    id: Option<String>,
    function: Option<OpenAiStreamFunction>,
}

#[derive(Deserialize)]
struct OpenAiStreamFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Default, Clone)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

fn parse_tool_calls(calls: Vec<OpenAiToolCall>) -> Vec<ToolCall> {
    calls
        .into_iter()
        .map(|c| {
            let arguments = serde_json::from_str(&c.function.arguments)
                .unwrap_or(json!({ "raw": c.function.arguments }));
            ToolCall {
                id: c.id,
                name: c.function.name,
                arguments,
            }
        })
        .collect()
}

fn finalize_pending_tools(pending: HashMap<usize, PendingToolCall>) -> Option<Vec<ToolCall>> {
    if pending.is_empty() {
        return None;
    }
    let mut indices: Vec<_> = pending.keys().copied().collect();
    indices.sort_unstable();
    let calls = indices
        .into_iter()
        .filter_map(|i| pending.get(&i).cloned())
        .map(|p| {
            let arguments =
                serde_json::from_str(&p.arguments).unwrap_or(json!({ "raw": p.arguments }));
            ToolCall {
                id: if p.id.is_empty() {
                    p.name.clone()
                } else {
                    p.id
                },
                name: p.name,
                arguments,
            }
        })
        .collect::<Vec<_>>();
    if calls.is_empty() {
        None
    } else {
        Some(calls)
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    fn name(&self) -> &str {
        &self.config.model_name
    }

    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<ChatMessage, AgentError> {
        let req_body = OpenAiChatRequest {
            model: &self.config.model_name,
            messages: Self::build_messages(messages),
            temperature: self.config.temperature,
            tools: Self::build_tools(tools),
            stream: false,
        };

        let mut req = self.client.post(self.endpoint()).json(&req_body);
        if let Some(ref key) = self.config.api_key {
            req = req.bearer_auth(key.expose_secret());
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AgentError::ProviderError(format!(
                "OpenAI returned HTTP {status}: {body}"
            )));
        }

        let chat_res: OpenAiChatResponse = resp.json().await?;
        let choice = chat_res
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| AgentError::ProviderError("Empty response from LLM".to_string()))?;

        let content = choice.message.content.unwrap_or_default();
        let tool_calls = choice.message.tool_calls.map(parse_tool_calls);

        Ok(ChatMessage {
            role: Role::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
        })
    }

    async fn stream(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        event_tx: mpsc::Sender<StreamEvent>,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<ChatMessage, AgentError> {
        if *cancel.borrow() {
            return Err(AgentError::ExecutionAborted);
        }

        let req_body = OpenAiChatRequest {
            model: &self.config.model_name,
            messages: Self::build_messages(messages),
            temperature: self.config.temperature,
            tools: Self::build_tools(tools),
            stream: true,
        };

        let mut req = self.client.post(self.endpoint()).json(&req_body);
        if let Some(ref key) = self.config.api_key {
            req = req.bearer_auth(key.expose_secret());
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AgentError::ProviderError(format!(
                "OpenAI stream returned HTTP {status}: {body}"
            )));
        }

        let mut byte_stream = resp.bytes_stream();
        let mut line_buf = String::new();
        let mut content = String::new();
        let mut pending_tools: HashMap<usize, PendingToolCall> = HashMap::new();

        loop {
            if *cancel.borrow() {
                return Err(AgentError::ExecutionAborted);
            }

            let chunk = tokio::select! {
                changed = cancel.changed() => {
                    if changed.is_ok() && *cancel.borrow() {
                        return Err(AgentError::ExecutionAborted);
                    }
                    continue;
                }
                next = byte_stream.next() => next,
            };

            let Some(next) = chunk else {
                break;
            };
            let bytes = next.map_err(AgentError::HttpError)?;
            line_buf.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(newline) = line_buf.find('\n') {
                let mut line = line_buf[..newline].to_string();
                line_buf = line_buf[newline + 1..].to_string();
                if line.ends_with('\r') {
                    line.pop();
                }
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data == "[DONE]" {
                    let tool_calls = finalize_pending_tools(pending_tools);
                    return Ok(ChatMessage {
                        role: Role::Assistant,
                        content,
                        tool_calls,
                        tool_call_id: None,
                    });
                }

                let chunk: OpenAiStreamChunk = serde_json::from_str(data).map_err(|e| {
                    AgentError::ProviderError(format!("invalid SSE chunk: {e}: {data}"))
                })?;
                let Some(choice) = chunk.choices.into_iter().next() else {
                    continue;
                };

                if let Some(delta) = choice.delta.content {
                    if !delta.is_empty() {
                        content.push_str(&delta);
                        let _ = event_tx.send(StreamEvent::Delta(delta)).await;
                    }
                }

                if let Some(tool_deltas) = choice.delta.tool_calls {
                    for td in tool_deltas {
                        let entry = pending_tools.entry(td.index).or_default();
                        if let Some(id) = td.id {
                            entry.id = id;
                        }
                        if let Some(func) = td.function {
                            if let Some(name) = func.name {
                                entry.name = name;
                            }
                            if let Some(args) = func.arguments {
                                entry.arguments.push_str(&args);
                            }
                        }
                    }
                }
            }
        }

        if *cancel.borrow() {
            return Err(AgentError::ExecutionAborted);
        }

        Ok(ChatMessage {
            role: Role::Assistant,
            content,
            tool_calls: finalize_pending_tools(pending_tools),
            tool_call_id: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalize_pending_tools_orders_by_index() {
        let mut pending = HashMap::new();
        pending.insert(
            1,
            PendingToolCall {
                id: "b".into(),
                name: "second".into(),
                arguments: r#"{"x":2}"#.into(),
            },
        );
        pending.insert(
            0,
            PendingToolCall {
                id: "a".into(),
                name: "first".into(),
                arguments: r#"{"x":1}"#.into(),
            },
        );
        let calls = finalize_pending_tools(pending).unwrap();
        assert_eq!(calls[0].name, "first");
        assert_eq!(calls[1].name, "second");
    }
}
