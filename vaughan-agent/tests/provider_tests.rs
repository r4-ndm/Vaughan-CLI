//! Unit tests for vaughan-agent types and provider serialization.

use secrecy::SecretString;
use serde_json::json;
use vaughan_agent::providers::create_llm_client;
use vaughan_agent::types::{ChatMessage, ModelConfig, Role, ToolCall, ToolDefinition};

#[test]
fn chat_message_constructors() {
    let sys = ChatMessage::system("You are a DeFi assistant.");
    assert_eq!(sys.role, Role::System);
    assert_eq!(sys.content, "You are a DeFi assistant.");

    let user = ChatMessage::user("Inspect 0x123");
    assert_eq!(user.role, Role::User);

    let tool_call = ToolCall {
        id: "call_1".to_string(),
        name: "inspect_contract".to_string(),
        arguments: json!({ "address": "0x123" }),
    };
    let assistant = ChatMessage::assistant_with_tools("Calling inspect", vec![tool_call]);
    assert_eq!(assistant.role, Role::Assistant);
    assert!(assistant.tool_calls.is_some());
    assert_eq!(assistant.tool_calls.as_ref().unwrap().len(), 1);

    let tool_resp =
        ChatMessage::tool_response("call_1", json!({ "name": "Wrapped Ether" }).to_string());
    assert_eq!(tool_resp.role, Role::Tool);
    assert_eq!(tool_resp.tool_call_id, Some("call_1".to_string()));
}

#[test]
fn tool_definition_serialization() {
    let tool = ToolDefinition {
        name: "get_reserves".to_string(),
        description: "Fetch reserves for a Uniswap V2 pair".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "pair_address": { "type": "string" }
            },
            "required": ["pair_address"]
        }),
    };

    let serialized = serde_json::to_string(&tool).unwrap();
    assert!(serialized.contains("get_reserves"));
    assert!(serialized.contains("pair_address"));
}

#[test]
fn create_providers_from_config() {
    let ollama_cfg = ModelConfig::default_local_ollama();
    let client = create_llm_client(ollama_cfg).unwrap();
    assert_eq!(client.name(), "llama3.2");

    let gemini_cfg = ModelConfig::gemini(
        SecretString::from("fake_key".to_string()),
        "gemini-2.5-flash",
    );
    let gemini_client = create_llm_client(gemini_cfg).unwrap();
    assert_eq!(gemini_client.name(), "gemini-2.5-flash");

    let openai_cfg = ModelConfig::openai(
        "https://api.openai.com",
        SecretString::from("fake_key".to_string()),
        "gpt-4o",
    );
    let openai_client = create_llm_client(openai_cfg).unwrap();
    assert_eq!(openai_client.name(), "gpt-4o");
}
