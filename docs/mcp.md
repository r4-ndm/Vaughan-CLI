# Vaughan MCP Setup

Vaughan exposes a **Model Context Protocol (MCP)** stdio server so external
agents (Cursor, Claude Code, Codex) can use the same DeFi verbs as the TUI.

**Two roles** — details: [`agent-roles.md`](agent-roles.md):

1. **Adviser** — you use Vaughan; agent proposes; you approve (`vaughan` /
   `--profile default`).
2. **Sentient** — the agent uses **its own seed** and acts with full control
   (`vaughan-sentient` / `--profile sentient`; auto-exec under policy — wiring
   in progress). Human may **partner** by sharing that same seed.

Signing never happens inside the MCP process. Keys stay in Vaughan.

See also:

- [`agent-roles.md`](agent-roles.md) — adviser vs sentient  
- [`ai-tool-surface.md`](ai-tool-surface.md) — tool contract  
- [`mcp-threat-model.md`](mcp-threat-model.md) — security controls  

## Cursor configuration

### Adviser (human-led) — `vaughan`

Project config ships at [`.cursor/mcp.json`](../.cursor/mcp.json) (cargo-run for
dev). For an installed binary:

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

### Sentient (agent-led) — `vaughan-sentient`

Separate MCP entry on the **agent’s** profile (not the human’s `default` savings
unless you intentionally share a seed for partnership):

```json
{
  "mcpServers": {
    "vaughan-sentient": {
      "command": "vaughan",
      "args": ["mcp", "--profile", "sentient"]
    }
  }
}
```

Keep `vaughan` and `vaughan-sentient` as two named servers so roles stay clear.
Legacy on-disk profile name `degen` still resolves as an alias of `sentient`
until migrated.

Build/install Vaughan first (`cargo install --path vaughan-cli`), or rely on the
project `.cursor/mcp.json` which runs `cargo run -p vaughan-cli -- mcp`.

## Architecture (v1)

- **TUI / Vaughan owns keys** — MCP never unlocks the vault.
- **Adviser path:** MCP → loopback → TUI approval card → sign.
- **Sentient path (target):** MCP → Vaughan sentient session → policy → auto-sign.
- **Offline adviser:** MCP writes `proposals/pending/*.json` → open Vaughan later → approve.
- **Partnership:** human and agent unlock the same seed → same funds, either may act.

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
2. Connect Vaughan MCP (`.cursor/mcp.json`; restart Cursor MCP).
3. `get_address` / `list_assets` — should return the unlocked account.
4. `propose_transfer` → **Deny** in TUI → `proposals/rejected/` has the file.
5. `propose_transfer` → **Approve** on testnet → status shows `tx_hash`.
6. Optional Pulse DeFi: `quote_swap` then `propose_agg_swap`.

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
