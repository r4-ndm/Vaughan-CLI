# MCP smoke checklist (humans + Cursor)

Manual verification that Vaughan’s MCP stdio subset works with a real host.
Automated wire-format tests: `cargo test -p vaughan-mcp --test conformance`.

## Claimed protocol subset

| Item | Vaughan |
|------|---------|
| Framing | **Newline-delimited JSON** (one object per line). Not Content-Length. |
| Methods | `initialize`, `notifications/initialized` / `initialized`, `ping`, `tools/list`, `tools/call` |
| Logs | **stderr only** — stdout is JSON-RPC responses |
| Protocol version | `2024-11-05` (`MCP_PROTOCOL_VERSION`) |
| Tool errors | `result.isError: true` + text content (not always a JSON-RPC `error` object) |

If a host requires Content-Length framing, it is **unsupported** until we document
a second transport mode.

## Prerequisites

1. Build or install `vaughan` (`cargo build -p vaughan-cli` or release install).
2. Cursor MCP (or Claude/Codex) configured per [`mcp.md`](mcp.md).
3. Prefer **PulseChain testnet** (943). Unlock Vaughan TUI **or** run
   `vaughan serve` for write paths.

## Automated (CI)

```bash
cargo test -p vaughan-mcp --test conformance
cargo test -p vaughan-mcp --test mcp_integration
cargo test -p vaughan-tui --test mcp_dogfood --test mcp_listener
```

Conformance covers: initialize shape, ping, tools/list catalog + banned names,
`get_network` / `get_control_plane_status` envelopes, unknown tool → `isError`,
method-not-found / parse-error JSON-RPC codes.

## Cursor smoke (adviser — `vaughan`)

Do these in order. Tick when green.

- [ ] MCP server connects without crashing (no stdout log noise).
- [ ] Host shows Vaughan tools (at least `get_network`, `propose_transfer`, `quote_swap`).
- [ ] Call **`get_network`** → returns `chain_id`, `network_id`, `rpc_url`.
- [ ] Call **`get_control_plane_status`** → see `ready_for_writes` true/false + `hint`.
- [ ] Unlock TUI (or `serve`) → `get_address` / `list_assets` return the account.
- [ ] **`propose_transfer`** tiny testnet amount → **Deny** in TUI → status rejected / file under `proposals/rejected/`.
- [ ] **`propose_transfer`** again → **Approve** on testnet → `tx_hash` / approved status.
- [ ] Optional: `quote_swap` then `propose_agg_swap` (testnet-safe venue).

## Cursor smoke (sentient — `vaughan-sentient`)

- [ ] Separate MCP entry on `--profile sentient` (not human `default` seed).
- [ ] Preset applied: `vaughan --profile sentient preset apply balanced`.
- [ ] Control plane up: unlocked TUI **or** `vaughan --profile sentient serve --password-env …`.
- [ ] `get_control_plane_status` → `sentient_auto_exec: true`, `ready_for_writes: true`.
- [ ] `watch_balance` / `watch_quote` return snapshots (no sign).
- [ ] One tiny `propose_transfer` auto-execs under policy (or fails clearly on breaker) — **testnet only**.

## Error vocabulary agents should handle

Stable substrings / codes (see also [`ai-tool-surface.md`](ai-tool-surface.md)):

| Signal | Meaning |
|--------|---------|
| `wallet_locked` | Unlock TUI/serve or pass `account_address` on reads |
| `tui_offline` / `session_required` | Start TUI or `vaughan serve` |
| `mainnet_blocked` | Set `VAUGHAN_MCP_ALLOW_MAINNET=1` only if intentional |
| `unknown tool` | Typo or tool not in this profile’s list |
| fee … 10% | Fee spike — re-propose |
| `isError: true` | Tool failed; read `content[0].text` |

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| Host hangs / no tools | Restart MCP; ensure `command` is `vaughan mcp` not a wrapper that logs to stdout |
| `ready_for_writes: false` | Unlock TUI or start `serve`; check `get_control_plane_status.hint` |
| Propose stuck `pending_user` | Adviser needs TUI open; sentient needs unlocked serve/TUI |
| Parse / method errors in conformance | Regression — open an issue with fixture name |

## Related

- [`mcp.md`](mcp.md) — setup  
- [`mcp-threat-model.md`](mcp-threat-model.md) — security  
- [`sentient-ops.md`](sentient-ops.md) — always-on serve  
- `vaughan-mcp/tests/fixtures/` — golden request lines  
