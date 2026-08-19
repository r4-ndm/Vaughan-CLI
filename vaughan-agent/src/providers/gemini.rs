//! Google Gemini Cloud provider client.

use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client::LlmClient;
use crate::error::AgentError;
use crate::types::{ChatMessage, ModelConfig, Role, ToolCall, ToolDefinition};

pub struct GeminiClient {
    config: ModelConfig,
    client: reqwest::Client,
}

impl GeminiClient {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Serialize)]
struct GeminiRequest<'a> {
    contents: Vec<GeminiContent<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<GeminiToolGroup<'a>>,
    #[serde(rename = "generationConfig")]
    generation_config: GeminiGenConfig,
}

#[derive(Serialize)]
struct GeminiGenConfig {
    temperature: f32,
}

#[derive(Serialize)]
struct GeminiContent<'a> {
    role: &'a str,
    parts: Vec<GeminiPart<'a>>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum GeminiPart<'a> {
    Text {
        text: &'a str,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCallRef<'a>,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse<'a>,
    },
}

#[derive(Serialize)]
struct GeminiFunctionCallRef<'a> {
    name: &'a str,
    args: &'a Value,
}

#[derive(Serialize)]
struct GeminiFunctionResponse<'a> {
    name: &'a str,
    response: Value,
}

#[derive(Serialize)]
struct GeminiToolGroup<'a> {
    #[serde(rename = "functionDeclarations")]
    function_declarations: Vec<GeminiFunctionDecl<'a>>,
}

#[derive(Serialize)]
struct GeminiFunctionDecl<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a Value,
}

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiResponseContent>,
}

#[derive(Deserialize)]
struct GeminiResponseContent {
    parts: Vec<GeminiResponsePart>,
}

#[derive(Deserialize)]
struct GeminiResponsePart {
    text: Option<String>,
    #[serde(rename = "functionCall")]
    function_call: Option<GeminiReceivedFunctionCall>,
}

#[derive(Deserialize)]
struct GeminiReceivedFunctionCall {
    name: String,
    args: Option<Value>,
}

#[async_trait]
impl LlmClient for GeminiClient {
    fn name(&self) -> &str {
        &self.config.model_name
    }

    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<ChatMessage, AgentError> {
        let api_key =
            self.config.api_key.as_ref().ok_or_else(|| {
                AgentError::ProviderError("Gemini requires an API key".to_string())
            })?;

        let endpoint = format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.config.endpoint_url.trim_end_matches('/'),
            self.config.model_name,
            api_key.expose_secret()
        );

        let contents: Vec<GeminiContent> = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    Role::System | Role::User => "user",
                    Role::Assistant => "model",
                    Role::Tool => "user",
                };
                let mut parts = Vec::new();
                if !m.content.is_empty() {
                    parts.push(GeminiPart::Text { text: &m.content });
                }
                if let Some(ref calls) = m.tool_calls {
                    for call in calls {
                        parts.push(GeminiPart::FunctionCall {
                            function_call: GeminiFunctionCallRef {
                                name: &call.name,
                                args: &call.arguments,
                            },
                        });
                    }
                }
                if let Some(ref tool_id) = m.tool_call_id {
                    let parsed: Value =
                        serde_json::from_str(&m.content).unwrap_or(json!({ "output": m.content }));
                    parts.push(GeminiPart::FunctionResponse {
                        function_response: GeminiFunctionResponse {
                            name: tool_id,
                            response: parsed,
                        },
                    });
                }
                GeminiContent { role, parts }
            })
            .collect();

        let tool_groups = if tools.is_empty() {
            Vec::new()
        } else {
            vec![GeminiToolGroup {
                function_declarations: tools
                    .iter()
                    .map(|t| GeminiFunctionDecl {
                        name: &t.name,
                        description: &t.description,
                        parameters: &t.parameters,
                    })
                    .collect(),
            }]
        };

        let req_body = GeminiRequest {
            contents,
            tools: tool_groups,
            generation_config: GeminiGenConfig {
                temperature: self.config.temperature,
            },
        };

        let resp = self.client.post(&endpoint).json(&req_body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AgentError::ProviderError(format!(
                "Gemini API returned HTTP {status}: {body}"
            )));
        }

        let gem_res: GeminiResponse = resp.json().await?;
        let candidate = gem_res
            .candidates
            .and_then(|c| c.into_iter().next())
            .ok_or_else(|| {
                AgentError::ProviderError("No response candidates returned by Gemini".to_string())
            })?;

        let parts = candidate.content.map(|c| c.parts).unwrap_or_default();

        let mut content = String::new();
        let mut tool_calls = Vec::new();

        for part in parts {
            if let Some(t) = part.text {
                content.push_str(&t);
            }
            if let Some(fc) = part.function_call {
                tool_calls.push(ToolCall {
                    id: fc.name.clone(),
                    name: fc.name,
                    arguments: fc.args.unwrap_or(json!({})),
                });
            }
        }

        Ok(ChatMessage {
            role: Role::Assistant,
            content,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            tool_call_id: None,
        })
    }
}
