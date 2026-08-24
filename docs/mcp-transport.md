# MCP transport: hand-rolled stdio vs `rmcp`

**Decision (2026-08-24): no `rmcp` rewrite now.** Treat migration as optional
maintenance, not a product or security requirement.

`rmcp` (or any official MCP Rust SDK) would only replace the **stdio / JSON-RPC
cable**. Fund-safety (re-sim, fee spike, HMAC queue, vault unlock, signing)
stays in Vaughan core / TUI / `serve` forever — never in the MCP process.

## Layers

| Layer | Owner today | After a future `rmcp` migrate |
|-------|-------------|-------------------------------|
| Stdio framing + `initialize` / `tools/*` / `ping` | [`vaughan-mcp/src/server.rs`](../vaughan-mcp/src/server.rs) | SDK |
| Tool dispatch | [`dispatch.rs`](../vaughan-mcp/src/dispatch.rs) | Same behavior, different wire registration |
| Session bridge tool defs | [`session_bridge.rs`](../vaughan-mcp/src/session_bridge.rs) | Unchanged list |
| DeFi tool schemas | `vaughan-agent` registries | Unchanged |
| Sign / fee spike / re-sim / HMAC | `vaughan-core` + TUI / `serve` | **Must stay here** |

Debug map (symptom → file → test): [`mcp-smoke.md`](mcp-smoke.md#break-map-symptom--file--test).

## Why not needed now

1. Cursor / Claude work with the claimed subset (newline JSON; see [`mcp-smoke.md`](mcp-smoke.md)).
2. Wire format is locked by `cargo test -p vaughan-mcp --test conformance`.
3. Fund-safety is tested separately (dogfood / listener / proposal unit tests).
4. Adding `rmcp` needs allowlist approval ([`CLAUDE.md`](../CLAUDE.md)), dual-run cost, and risk to stdout purity.
5. Product backlog (intent macros, browser writes, demo, UX) beats protocol cosmetics.

## Keep the cable thin (hard rules)

- MCP **never** unlocks the vault or holds signers.
- MCP **never** implements fee spike / re-sim / HMAC — those run at approve / auto-exec.
- Logs go to **stderr only**; stdout is JSON-RPC lines only.
- Prefer registry-backed tools; keep session tools in [`session_bridge.rs`](../vaughan-mcp/src/session_bridge.rs) small.

## Revisit triggers (any one is enough)

Schedule a spike **only if**:

- A major host (Cursor / Claude / Codex) **breaks** on newline framing or requires Content-Length / newer MCP methods we refuse to hand-roll.
- Protocol churn forces more time in `server.rs` than shipping product.
- Official SDK features (resources, prompts, sampling) become product requirements for Vaughan.

Until then: **do not schedule a rewrite.** Prefer the smallest hand-rolled fix first (e.g. optional Content-Length framing in `server.rs`).

## If you ever migrate (spike recipe)

1. Ask to allowlist the crate in workspace deps.
2. Spike: empty `rmcp` stdio server + **one** read tool (`get_network`) calling existing `McpDispatcher`.
3. Side-by-side: keep `vaughan mcp` (hand-rolled) and a flag / binary for the SDK path until Cursor smoke ([`mcp-smoke.md`](mcp-smoke.md)) matches.
4. Port `tools/list` + `tools/call` only; leave IPC + signing untouched.
5. Delete hand-rolled stdio only after hosts stay green for a release cycle.
6. **Never** fold vault unlock or signing into the MCP process.

## Related

- [`mcp.md`](mcp.md) — setup  
- [`mcp-smoke.md`](mcp-smoke.md) — smoke + conformance  
- [`mcp-threat-model.md`](mcp-threat-model.md) — security  
- [`sentient-ops.md`](sentient-ops.md) — always-on ops  
- [`TASKS.md`](../TASKS.md) — explicitly out of scope until revisit triggers fire  
