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

Agent playbook (connect + reconnect): [`vaughan-agent/skills/mcp-connect/SKILL.md`](../vaughan-agent/skills/mcp-connect/SKILL.md) · per-host: [`hosts/INDEX.md`](../vaughan-agent/skills/mcp-connect/hosts/INDEX.md).

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
2. Connect Vaughan MCP (`.cursor/mcp.json`; reconnect host — see [`mcp-connect` skill](../vaughan-agent/skills/mcp-connect/SKILL.md)).
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
| MCP broken in Cursor | Ensure nothing writes to stdout except JSON-RPC (logs go to stderr); reconnect MCP — [`mcp-connect`](../vaughan-agent/skills/mcp-connect/SKILL.md) |

## Browser tools (VB B1 + B2)

Optional `vaughan-dapp-browser` control for agents. **Never signs** — signing stays
in the TUI/provider.

**CDP is off by default (FR-7.5).** Enable agent browser control before B2 tools:

- TUI: Settings (`n`) → **`p`**
- CLI: `vaughan config agent-browser on`
- Override: `VAUGHAN_DAPP_BROWSER_CDP_PORT=9222` in MCP host env (smoke/dev)

When enabled, each VB spawn gets a **random loopback CDP port** (the env
override pins a fixed port for dev). Chrome CDP itself has no authentication —
the endpoint is guarded instead by: PID-bound `vb.session` (MCP verifies the
recorded PID is a live `vaughan-dapp-browser` before any call), a pinned tab
target (`vb.target`), and an allowlist re-check of the current page URL before
mutating tools (`browser_click`/`browser_type`/`browser_connect_wallet`/…).
`data:` and `blob:` navigation targets are rejected; `browser_snapshot` masks
input field values; wallet auto-connect is refused on public IPFS gateways.

When the wallet is unlocked, the **F1 network strip** shows **`· MCP on`** when the loopback
control plane is listening (`127.0.0.1:8746`). **`· MCP off`** means bind failed (often
`vaughan serve` still running) — stop serve and lock/unlock the TUI.

Kill-switch: [vb-kill-switch.md](vb-kill-switch.md).

| Tool | Purpose |
|------|---------|
| `browser_open` | Spawn VB at allowlisted `url`; CDP only when control enabled (see above) |
| `browser_open_agg` | Ag-catalog swap UI — venue id + optional `pls_hex`; playbook: [`vb-ag-quotes/SKILL.md`](../vaughan-agent/skills/vb-ag-quotes/SKILL.md) |
| `browser_navigate` | CDP navigate to allowlisted `url` (checks `vb.session` suffixes) |
| `browser_status` | CDP health + `agent_browser_control` + open page URLs |

When VB is missing or CDP is down, tools return structured JSON with
`available: false` and a `hint` (not a crash).

**B2 navigation** (requires live CDP session from `browser_open`):

| Tool | Purpose |
|------|---------|
| `browser_snapshot` | Interactive refs `e0`…`e49` (title, url, tag, role, name) |
| `browser_click` | Click ref from snapshot |
| `browser_type` | Focus ref + insert text |
| `browser_press` | Key press (Enter, Tab, Escape, arrows, Space, Backspace, Delete) |
| `browser_wait` | Poll until `text`, `selector`, or `url_contains` matches |

Requires `vaughan-dapp-browser` on `PATH`. In-tab navigation is also gated inside
the Chromium extension (MV3 allowlist).

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
- **Sentient:** auto-exec under policy (same as unlocked TUI). Treat the
  host as a **hot wallet** — any same-user process with the session token can spend.
  Unlimited ERC-20 `propose_approve` also bypasses native size breakers (zero value).
- **Default:** queues `pending_user` for normal proposes. Stealth **sweep** still
  needs an unlocked TUI on adviser (serve returns `tui_required`).
- Mainnet writes still require `VAUGHAN_MCP_ALLOW_MAINNET=1` (same gate as MCP stdio).
- EmpX quotes/proposes are PulseChain **mainnet (369) only**.
- MCP stdio (`vaughan mcp`) remains the agent-facing process; it attaches to the
  serve/TUI socket.

Ctrl-C stops the daemon and invalidates the session token.
