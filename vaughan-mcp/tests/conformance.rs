//! MCP stdio JSON-RPC conformance tests (Cursor-compatible subset).
//!
//! Fixtures live in `tests/fixtures/`. These do **not** require Anvil or an
//! unlocked TUI — they lock the wire format and tool catalog shape.
//!
//! Manual host smoke: [`docs/mcp-smoke.md`](../../docs/mcp-smoke.md).

use serde_json::{json, Value};
use vaughan_mcp::{build_context, handle_stdio_line, McpDispatcher, MCP_PROTOCOL_VERSION};

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {path}: {e}"))
        .trim()
        .to_string()
}

fn fixture_json(name: &str) -> Value {
    serde_json::from_str(&fixture(name)).expect("fixture JSON")
}

async fn harness() -> (McpDispatcher, vaughan_mcp::McpContext) {
    let profile = "default";
    let dispatcher = McpDispatcher::new(profile).expect("dispatcher");
    let ctx = build_context(profile, "conformance").expect("context");
    (dispatcher, ctx)
}

fn content_text(resp: &vaughan_mcp::RpcResponse) -> String {
    let result = resp.result.as_ref().expect("expected result");
    result["content"][0]["text"]
        .as_str()
        .expect("content text")
        .to_string()
}

#[tokio::test]
async fn initialize_advertises_protocol_and_tools_capability() {
    let (dispatcher, ctx) = harness().await;
    let resp = handle_stdio_line(&dispatcher, &ctx, &fixture("initialize_request.jsonl")).await;
    assert!(resp.error.is_none(), "{resp:?}");
    let result = resp.result.expect("result");
    assert_eq!(result["protocolVersion"], MCP_PROTOCOL_VERSION);
    assert_eq!(result["serverInfo"]["name"], "vaughan-mcp");
    assert!(result["serverInfo"]["version"].as_str().is_some());
    assert!(result["capabilities"]["tools"].is_object());
    assert_eq!(resp.jsonrpc, "2.0");
    assert_eq!(resp.id, json!(1));
}

#[tokio::test]
async fn ping_returns_empty_object() {
    let (dispatcher, ctx) = harness().await;
    let resp = handle_stdio_line(&dispatcher, &ctx, &fixture("ping_request.jsonl")).await;
    assert!(resp.error.is_none());
    assert_eq!(resp.result, Some(json!({})));
    assert_eq!(resp.id, json!(2));
}

#[tokio::test]
async fn tools_list_shape_and_catalog() {
    let (dispatcher, ctx) = harness().await;
    let resp = handle_stdio_line(&dispatcher, &ctx, &fixture("tools_list_request.jsonl")).await;
    assert!(resp.error.is_none(), "{resp:?}");
    let tools = resp.result.expect("result")["tools"]
        .as_array()
        .expect("tools array")
        .clone();
    assert!(!tools.is_empty(), "tools/list must not be empty");

    let mut names = Vec::new();
    for tool in &tools {
        let name = tool["name"].as_str().expect("name");
        assert!(!name.is_empty());
        assert!(
            tool["description"]
                .as_str()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "tool {name} missing description"
        );
        let schema = &tool["inputSchema"];
        assert_eq!(
            schema["type"].as_str(),
            Some("object"),
            "tool {name} inputSchema.type"
        );
        names.push(name.to_string());
    }

    let required: Vec<String> =
        serde_json::from_value(fixture_json("required_tools.json")).unwrap();
    for req in &required {
        assert!(
            names.iter().any(|n| n == req),
            "tools/list missing required tool: {req}"
        );
    }

    let banned: Vec<String> =
        serde_json::from_value(fixture_json("banned_tool_substrings.json")).unwrap();
    for b in &banned {
        assert!(
            !names.iter().any(|n| n.contains(b.as_str())),
            "banned substring '{b}' leaked into tools/list"
        );
    }
}

#[tokio::test]
async fn tools_call_get_network_success_envelope() {
    let (dispatcher, ctx) = harness().await;
    let resp = handle_stdio_line(&dispatcher, &ctx, &fixture("tools_call_get_network.jsonl")).await;
    assert!(resp.error.is_none(), "{resp:?}");
    let result = resp.result.as_ref().expect("result");
    assert_eq!(result["isError"], false);
    assert_eq!(result["content"][0]["type"], "text");
    let text = content_text(&resp);
    let body: Value = serde_json::from_str(&text).expect("tool JSON body");
    assert!(body.get("chain_id").and_then(|v| v.as_u64()).is_some());
    assert!(body.get("network_id").and_then(|v| v.as_str()).is_some());
    assert!(body.get("rpc_url").and_then(|v| v.as_str()).is_some());
}

#[tokio::test]
async fn tools_call_control_plane_status_envelope() {
    let (dispatcher, ctx) = harness().await;
    let resp = handle_stdio_line(
        &dispatcher,
        &ctx,
        &fixture("tools_call_control_plane.jsonl"),
    )
    .await;
    assert!(resp.error.is_none(), "{resp:?}");
    let result = resp.result.as_ref().expect("result");
    assert_eq!(result["isError"], false);
    let body: Value = serde_json::from_str(&content_text(&resp)).unwrap();
    assert!(body.get("control_plane_reachable").is_some());
    assert!(body.get("ready_for_writes").is_some());
    assert!(body.get("hint").and_then(|v| v.as_str()).is_some());
}

#[tokio::test]
async fn tools_call_unknown_sets_is_error() {
    let (dispatcher, ctx) = harness().await;
    let resp = handle_stdio_line(&dispatcher, &ctx, &fixture("tools_call_unknown.jsonl")).await;
    assert!(
        resp.error.is_none(),
        "tool errors use result.isError, not JSON-RPC error"
    );
    let result = resp.result.as_ref().expect("result");
    assert_eq!(result["isError"], true);
    let text = content_text(&resp);
    assert!(
        text.contains("unknown tool") || text.contains("not_a_real_tool"),
        "unexpected error text: {text}"
    );
}

#[tokio::test]
async fn method_not_found_is_jsonrpc_error() {
    let (dispatcher, ctx) = harness().await;
    let resp = handle_stdio_line(&dispatcher, &ctx, &fixture("method_not_found.jsonl")).await;
    assert!(resp.result.is_none());
    let err = resp.error.expect("error");
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("method not found"));
}

#[tokio::test]
async fn parse_error_is_jsonrpc_parse_error() {
    let (dispatcher, ctx) = harness().await;
    let resp = handle_stdio_line(&dispatcher, &ctx, &fixture("parse_error.jsonl")).await;
    assert_eq!(resp.id, Value::Null);
    let err = resp.error.expect("error");
    assert_eq!(err.code, -32700);
    assert!(err.message.contains("parse error"));
}

#[tokio::test]
async fn notifications_initialized_accepted() {
    let (dispatcher, ctx) = harness().await;
    let line = r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#;
    let resp = handle_stdio_line(&dispatcher, &ctx, line).await;
    assert!(resp.error.is_none());
    assert_eq!(resp.result, Some(json!({})));
}

#[tokio::test]
async fn browser_status_unavailable_without_vb_session() {
    // Isolate from any live VB session on the host machine.
    let tmp = tempfile::tempdir().expect("tempdir");
    std::env::set_var("VAUGHAN_VB_STATE_DIR", tmp.path());
    let (_dispatcher, ctx) = harness().await;
    let body = vaughan_mcp::browser_bridge::browser_status(&ctx)
        .await
        .expect("browser_status should return JSON, not hard error");
    assert_eq!(body["available"], false);
    assert_eq!(body["reason"], "no_vb_session");
    assert!(body["hint"].as_str().is_some());
    assert!(body.get("agent_browser_control").is_some());
    std::env::remove_var("VAUGHAN_VB_STATE_DIR");
}

#[tokio::test]
async fn browser_navigate_blocked_when_agent_control_off() {
    std::env::remove_var("VAUGHAN_DAPP_BROWSER_CDP_PORT");
    // Hermetic: a never-created profile has no persisted toggle, so agent
    // browser control is off regardless of the developer's real wallet state.
    let profile = "conformance-no-such-profile";
    let ctx = build_context(profile, "conformance").expect("context");
    let err = vaughan_mcp::browser_bridge::browser_navigate(
        serde_json::json!({ "url": "https://example.com" }),
        &ctx,
    )
    .await
    .expect_err("navigate should fail without agent control");
    assert!(err.contains("agent browser control disabled"));
}
