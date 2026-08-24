//! Session / control-plane MCP tools (not in `vaughan-agent` registries).
//!
//! These bridge Vaughan TUI / `vaughan serve` (loopback IPC + proposal queue).
//! Keep this list small — DeFi verbs belong in the agent registries.

use serde_json::{json, Value};

fn empty_object_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}

/// MCP `tools/list` entries for session bridge tools (stdio-facing).
pub fn session_bridge_tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "get_address",
            "Active wallet address when Vaughan TUI is unlocked (session bridge).",
            empty_object_schema(),
        ),
        tool(
            "get_network",
            "Active network id, chain id, and RPC for the MCP session.",
            empty_object_schema(),
        ),
        tool(
            "list_assets",
            "Native + known ERC-20 balances for the unlocked active account.",
            empty_object_schema(),
        ),
        tool(
            "get_proposal_status",
            "Get lifecycle status of a pending or completed proposal.",
            json!({
                "type": "object",
                "properties": {
                    "proposal_id": { "type": "string" }
                },
                "required": ["proposal_id"]
            }),
        ),
        tool(
            "list_pending_proposals",
            "List all pending proposals in the file queue.",
            empty_object_schema(),
        ),
        tool(
            "get_control_plane_status",
            "Whether Vaughan TUI or `vaughan serve` is reachable on loopback, \
             and whether the wallet session is unlocked. Sentients should poll this before writes.",
            empty_object_schema(),
        ),
        tool(
            "get_stealth_uri",
            "This vault's ERC-5564 stealth meta-address URI (st:…). Requires unlocked TUI or vaughan serve.",
            empty_object_schema(),
        ),
        tool(
            "scan_stealth_notes",
            "Scan for unswept stealth notes owned by this vault. Requires unlocked TUI or vaughan serve.",
            empty_object_schema(),
        ),
        tool(
            "sweep_stealth_note",
            "Sweep one stealth note to the active account (approval card on adviser; auto on sentient).",
            json!({
                "type": "object",
                "properties": {
                    "stealth_address": { "type": "string" }
                },
                "required": ["stealth_address"]
            }),
        ),
    ]
}

/// Names of session-bridge tools (for tests / docs; dispatch routes explicitly).
pub fn session_bridge_tool_names() -> &'static [&'static str] {
    &[
        "get_address",
        "get_network",
        "list_assets",
        "get_proposal_status",
        "list_pending_proposals",
        "get_control_plane_status",
        "get_stealth_uri",
        "scan_stealth_notes",
        "sweep_stealth_note",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_defs_have_stable_shape() {
        let defs = session_bridge_tool_definitions();
        assert_eq!(defs.len(), session_bridge_tool_names().len());
        for t in &defs {
            let name = t["name"].as_str().unwrap();
            assert!(
                session_bridge_tool_names().contains(&name),
                "unexpected bridge tool {name}"
            );
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }
}
