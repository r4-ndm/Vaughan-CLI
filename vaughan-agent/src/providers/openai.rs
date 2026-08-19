//! OpenAI-compatible LLM client (works with OpenAI, DeepSeek, LocalAI, vLLM).

use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client::LlmClient;
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
}

#[derive(Serialize)]
struct OpenAiChatRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAiMessage<'a>>,
    temperature: f32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiTool<'a>>,
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
        let endpoint = format!(
            "{}/v1/chat/completions",
            self.config.endpoint_url.trim_end_matches('/')
        );

        let oai_messages: Vec<OpenAiMessage> = messages
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
            .collect();

        let oai_tools: Vec<OpenAiTool> = tools
            .iter()
            .map(|t| OpenAiTool {
                tool_type: "function",
                function: OpenAiFunctionDef {
                    name: &t.name,
                    description: &t.description,
                    parameters: &t.parameters,
                },
            })
            .collect();

        let req_body = OpenAiChatRequest {
            model: &self.config.model_name,
            messages: oai_messages,
            temperature: self.config.temperature,
            tools: oai_tools,
        };

        let mut req = self.client.post(&endpoint).json(&req_body);
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
        let tool_calls = choice.message.tool_calls.map(|calls| {
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
        });

        Ok(ChatMessage {
            role: Role::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
        })
    }
}
