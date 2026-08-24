# MCP stdio conformance fixtures

Newline-delimited JSON-RPC request samples for `tests/conformance.rs`.

| File | Asserts |
|------|---------|
| `initialize_request.jsonl` | protocolVersion, serverInfo, tools capability |
| `ping_request.jsonl` | empty result object |
| `tools_list_request.jsonl` | tool schema shape + required/banned lists |
| `tools_call_get_network.jsonl` | success envelope (`isError: false`) |
| `tools_call_control_plane.jsonl` | control-plane status fields |
| `tools_call_unknown.jsonl` | `isError: true` |
| `method_not_found.jsonl` | JSON-RPC `-32601` |
| `parse_error.jsonl` | JSON-RPC `-32700` |
| `required_tools.json` | names that must appear in `tools/list` |
| `banned_tool_substrings.json` | substrings that must never appear |

Manual checklist: [`docs/mcp-smoke.md`](../../../docs/mcp-smoke.md).
