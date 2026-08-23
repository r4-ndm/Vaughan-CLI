//! Convert between Vaughan chat types and `genai` request/response shapes.

use std::collections::HashMap;

use genai::chat::{
    ChatMessage as GenChatMessage, ChatRequest, ContentPart, MessageContent, Tool,
    ToolCall as GenToolCall, ToolResponse,
};

use crate::types::{ChatMessage, Role, ToolCall, ToolDefinition};

/// Build a genai [`ChatRequest`] from Vaughan history + tool schemas.
pub(super) fn build_chat_request(
    messages: &[ChatMessage],
    tools: &[ToolDefinition],
) -> ChatRequest {
    let mut system = String::new();
    let mut gen_messages = Vec::new();
    // call_id → fn_name (Gemini functionResponse needs the name).
    let mut call_names: HashMap<String, String> = HashMap::new();

    for msg in messages {
        match msg.role {
            Role::System => {
                if !system.is_empty() {
                    system.push('\n');
                }
                system.push_str(&msg.content);
            }
            Role::User => {
                gen_messages.push(GenChatMessage::user(msg.content.clone()));
            }
            Role::Assistant => {
                let mut parts = Vec::new();
                if !msg.content.is_empty() {
                    parts.push(ContentPart::Text(msg.content.clone()));
                }
                if let Some(ref calls) = msg.tool_calls {
                    // Gemini 3: thought signatures must precede tool calls.
                    if let Some(first) = calls.first() {
                        if let Some(ref sig) = first.thought_signature {
                            parts.push(ContentPart::ThoughtSignature(sig.clone()));
                        }
                    }
                    for call in calls {
                        call_names.insert(call.id.clone(), call.name.clone());
                        parts.push(ContentPart::ToolCall(to_gen_tool_call(call)));
                    }
                }
                if parts.is_empty() {
                    parts.push(ContentPart::Text(String::new()));
                }
                gen_messages.push(GenChatMessage::assistant(MessageContent::from_parts(parts)));
            }
            Role::Tool => {
                let call_id = msg.tool_call_id.clone().unwrap_or_default();
                let fn_name = call_names.get(&call_id).cloned();
                let mut resp = ToolResponse::new(call_id, msg.content.clone());
                if let Some(name) = fn_name {
                    resp = resp.with_fn_name(name);
                }
                gen_messages.push(GenChatMessage::from(resp));
            }
        }
    }

    let mut req = ChatRequest::new(gen_messages);
    if !system.is_empty() {
        req = req.with_system(system);
    }
    if !tools.is_empty() {
        req = req.with_tools(tools.iter().map(to_gen_tool));
    }
    req
}

/// Map genai assistant content (text + tool calls + thought signatures) into Vaughan.
pub(super) fn chat_message_from_genai_content(content: MessageContent) -> ChatMessage {
    let part_thoughts: Vec<String> = content
        .thought_signatures()
        .into_iter()
        .map(str::to_string)
        .collect();
    let text = content.texts().join("");
    let gen_calls: Vec<GenToolCall> = content.into_tool_calls();

    let tool_calls = if gen_calls.is_empty() {
        None
    } else {
        Some(
            gen_calls
                .into_iter()
                .enumerate()
                .map(|(i, tc)| {
                    let mut call = from_gen_tool_call(&tc);
                    if i == 0 {
                        call.thought_signature = tc
                            .thought_signatures
                            .as_ref()
                            .and_then(|v| v.first().cloned())
                            .or_else(|| part_thoughts.first().cloned());
                    }
                    call
                })
                .collect(),
        )
    };

    ChatMessage {
        role: Role::Assistant,
        content: text,
        tool_calls,
        tool_call_id: None,
    }
}

fn to_gen_tool(def: &ToolDefinition) -> Tool {
    Tool::new(def.name.clone())
        .with_description(def.description.clone())
        .with_schema(def.parameters.clone())
}

fn to_gen_tool_call(call: &ToolCall) -> GenToolCall {
    GenToolCall {
        call_id: call.id.clone(),
        fn_name: call.name.clone(),
        fn_arguments: call.arguments.clone(),
        thought_signatures: call.thought_signature.clone().map(|s| vec![s]),
    }
}

fn from_gen_tool_call(tc: &GenToolCall) -> ToolCall {
    ToolCall {
        id: tc.call_id.clone(),
        name: tc.fn_name.clone(),
        arguments: tc.fn_arguments.clone(),
        thought_signature: tc
            .thought_signatures
            .as_ref()
            .and_then(|v| v.first().cloned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use genai::chat::ChatRole;
    use serde_json::json;

    #[test]
    fn request_echoes_thought_signature_before_tool_call() {
        let history = vec![
            ChatMessage::system("rules"),
            ChatMessage::user("balance?"),
            ChatMessage {
                role: Role::Assistant,
                content: String::new(),
                tool_calls: Some(vec![ToolCall {
                    id: "1".into(),
                    name: "get_balance".into(),
                    arguments: json!({"account_address": "0xabc"}),
                    thought_signature: Some("sig-xyz".into()),
                }]),
                tool_call_id: None,
            },
            ChatMessage::tool_response("1", r#"{"balance_wei":"0"}"#),
        ];
        let req = build_chat_request(&history, &[]);
        assert_eq!(req.system.as_deref(), Some("rules"));
        let assistant = req
            .messages
            .iter()
            .find(|m| m.role == ChatRole::Assistant)
            .expect("assistant turn");
        let parts = assistant.content.parts();
        assert!(matches!(parts.first(), Some(ContentPart::ThoughtSignature(s)) if s == "sig-xyz"));
        assert!(parts.iter().any(|p| p.is_tool_call()));
    }

    #[test]
    fn response_maps_tool_calls_and_text() {
        let content = MessageContent::from_parts(vec![
            ContentPart::ThoughtSignature("tok".into()),
            ContentPart::Text("checking…".into()),
            ContentPart::ToolCall(GenToolCall {
                call_id: "c1".into(),
                fn_name: "inspect_contract".into(),
                fn_arguments: json!({"address": "0x1"}),
                thought_signatures: Some(vec!["tok".into()]),
            }),
        ]);
        let msg = chat_message_from_genai_content(content);
        assert_eq!(msg.content, "checking…");
        let calls = msg.tool_calls.expect("tools");
        assert_eq!(calls[0].name, "inspect_contract");
        assert_eq!(calls[0].thought_signature.as_deref(), Some("tok"));
    }
}
