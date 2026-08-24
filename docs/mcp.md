# Vaughan MCP Setup

Vaughan exposes a **Model Context Protocol (MCP)** stdio server so external
agents (Cursor, Claude Code, Codex) can use the same DeFi verbs as the TUI.

**Two roles** — details: [`agent-roles.md`](agent-roles.md):

1. **Adviser** — you use Vaughan; agent proposes; you approve (`vaughan` /
   `--profile default`).
2. **Sentient** — the agent uses **its own seed** and acts with full control
   (`vaughan-sentient` / `--profile sentient`). Unlock the **sentient** TUI
   session; MCP proposals auto-exec (re-sim + policy). Human may **partner** by
   sharing that same seed + a skill preset.

Signing never happens inside the MCP process. Keys stay in Vaughan.

See also:

- [`agent-roles.md`](agent-roles.md) — adviser vs sentient  
- [`sentient-ops.md`](sentient-ops.md) — always-on serve, watch loops, isolation limits  
- [`mcp-smoke.md`](mcp-smoke.md) — Cursor smoke checklist + conformance test how-to  
- [`mcp-transport.md`](mcp-transport.md) — hand-rolled vs `rmcp` decision (no rewrite now)  
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

Separate MCP entry on the **agent’s** profile. Unlock Vaughan with
`--profile sentient` so the loopback session can auto-sign:

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

Install a behavior pack first: `vaughan --profile sentient preset apply balanced`.

Keep `vaughan` and `vaughan-sentient` as two named servers so roles stay clear.
Legacy on-disk profile name `degen` still triggers the same auto-exec path.

## Architecture (v1)

- **TUI / Vaughan owns keys** — MCP never unlocks the vault.
- **Adviser path:** MCP → loopback → TUI approval card → sign.
- **Sentient path:** MCP → Vaughan sentient session (unlocked) → policy → auto-sign.
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

Full tick-list (adviser + sentient): [`mcp-smoke.md`](mcp-smoke.md).

Quick path:

1. Unlock Vaughan TUI on PulseChain testnet v4.
2. Connect Vaughan MCP (`.cursor/mcp.json`; restart Cursor MCP).
3. `get_address` / `list_assets` — should return the unlocked account.
4. `propose_transfer` → **Deny** in TUI → `proposals/rejected/` has the file.
5. `propose_transfer` → **Approve** on testnet → status shows `tx_hash`.
6. Optional Pulse DeFi: `quote_swap` then `propose_agg_swap`.

Wire-format CI: `cargo test -p vaughan-mcp --test conformance`.

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `wallet_locked` | Unlock Vaughan TUI; or pass explicit `account_address` on read tools |
| `pending_user` forever | TUI must be open and unlocked for live socket path |
| `mainnet_blocked` | Set `VAUGHAN_MCP_ALLOW_MAINNET=1` (use testnet first) |
| MCP broken in Cursor | Ensure nothing writes to stdout except JSON-RPC (logs go to stderr) |

## v2 — `vaughan serve`

Minimal headless daemon (same MCP IPC wire as v1):

```bash
export VAUGHAN_WALLET_PASSWORD='…'   # never commit; use a secret manager
vaughan --profile sentient serve --password-env VAUGHAN_WALLET_PASSWORD
# or adviser (queues pending_user; no auto-sign without TUI):
vaughan serve --password-env VAUGHAN_WALLET_PASSWORD
```

- Unlocks the profile vault non-interactively, writes the session token, binds
  loopback MCP control (`127.0.0.1:8746`).
- Reads the password from `--password-env` once, then **unsets** that env var in
  the process (still prefer a short-lived secret injection; do not leave passwords
  in shell history or shared hosts).
- **Sentient / degen:** auto-exec under policy (same as unlocked TUI). Treat the
  host as a **hot wallet** — any same-user process with the session token can spend.
  Unlimited ERC-20 `propose_approve` also bypasses native size breakers (zero value).
- **Default:** queues `pending_user` for normal proposes. Stealth **sweep** still
  needs an unlocked TUI on adviser (serve returns `tui_required`).
- Mainnet writes still require `VAUGHAN_MCP_ALLOW_MAINNET=1` (same gate as MCP stdio).
- EmpX quotes/proposes are PulseChain **mainnet (369) only**.
- MCP stdio (`vaughan mcp`) remains the agent-facing process; it attaches to the
  serve/TUI socket.

Ctrl-C stops the daemon and invalidates the session token.
