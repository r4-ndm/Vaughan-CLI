# Vaughan MCP Setup

Vaughan exposes a **Model Context Protocol (MCP)** stdio server so external agents
(Cursor, Claude Code, Codex) can inspect chains and **propose** transactions.
Signing always happens in the Vaughan TUI after explicit human approval.

See also:

- [`ai-tool-surface.md`](ai-tool-surface.md) — tool contract
- [`mcp-threat-model.md`](mcp-threat-model.md) — security controls

## Cursor configuration

Add to `~/.cursor/mcp.json` (or project `.cursor/mcp.json`):

```json
{
  "mcpServers": {
    "vaughan": {
      "command": "vaughan",
      "args": ["mcp", "--profile", "default"]
    }
  }
}
```

Build/install Vaughan first (`cargo install --path vaughan-cli`).

## Architecture (v1)

- **TUI owns keys** — MCP never unlocks the vault.
- **Live path:** MCP → loopback `127.0.0.1:8746` → TUI approval card → sign.
- **Offline path:** MCP writes `proposals/pending/*.json` → open Vaughan later → approve.

## CLI JSON (without MCP)

```bash
vaughan balance --json
vaughan assets --json
vaughan networks --json
vaughan propose transfer 0x… 1000000000000000000 --json
vaughan proposals list --json
vaughan proposals show prop_12345 --json
```

## Success test (testnet 943)

1. Unlock Vaughan TUI on PulseChain testnet v4.
2. In Cursor: *"Check my testnet balance and draft 0.01 tPLS to `0x…`."*
3. Agent calls read tools → `propose_transfer` → **one approval card** in TUI.
4. Approve → `get_proposal_status` returns `tx_hash`.
5. Confirm: no keys in the MCP process; tx matches decoded calldata.

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `wallet_locked` | Unlock Vaughan TUI; or pass explicit `account_address` on read tools |
| `pending_user` forever | TUI must be open and unlocked for live socket path |
| `mainnet_blocked` | Set `VAUGHAN_MCP_ALLOW_MAINNET=1` (use testnet first) |
| MCP broken in Cursor | Ensure nothing writes to stdout except JSON-RPC (logs go to stderr) |

## v2 (deferred)

Long-running `vaughan serve` wallet daemon — TUI/MCP/CLI become thin clients.
v1 IPC types become the daemon wire protocol.
