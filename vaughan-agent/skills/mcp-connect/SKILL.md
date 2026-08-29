---
name: mcp-connect
description: >-
  Connect Vaughan MCP to Cursor, Claude Desktop, or Claude Code. Use when MCP tools
  are missing, wallet_locked, reconnect/restart MCP after config change, first-time
  setup, browser_* unavailable, or get_control_plane_status fails. Host steps:
  hosts/INDEX.md
mode: all
kind: guide
---

# MCP connect

> **Any agent:** wire Vaughan into the user's MCP host, verify the connection, then
> hand off to task skills (`vb-ag-quotes`, `dapp-connect`, …).
>
> **Host-specific UI steps:** [`hosts/INDEX.md`](hosts/INDEX.md)

## When to read this skill

Load this playbook when the user (or task) mentions:

- First-time Vaughan + Cursor / Claude / Codex setup
- MCP tools missing (`browser_open_agg`, `quote_swap`, `get_network`, …)
- “Restart MCP”, “reconnect vaughan server”, “toggle MCP”
- `wallet_locked`, `control_plane_reachable: false`, `tui_offline`
- After editing `.cursor/mcp.json`, `vaughan config agent-browser`, or rebuilding `vaughan`

**Read this before** [`vb-ag-quotes`](../vb-ag-quotes/SKILL.md) — quotes assume MCP is connected.

## Two cables (do not confuse)

| Cable | What it is | Symptom if broken |
|-------|------------|-------------------|
| **stdio MCP** | Host (Cursor) ↔ `vaughan mcp` child process | No Vaughan tools in agent; MCP server red/disconnected |
| **loopback IPC** | `vaughan mcp` ↔ unlocked TUI or `vaughan serve` | Tools exist but `wallet_locked`, `ready_for_writes: false` |

Fix cable 1 with host reconnect (see [`hosts/`](hosts/)). Fix cable 2 by unlocking Vaughan (TUI or `serve`).

## Roles (pick one MCP server name)

| Role | MCP name | Profile | Who signs |
|------|----------|---------|-----------|
| **Adviser** (default) | `vaughan` | `default` | Human approves in TUI |
| **Sentient** | `vaughan-sentient` | `sentient` | Agent auto-exec under policy |

Details: [`docs/agent-roles.md`](../../docs/agent-roles.md). Never point `vaughan-sentient` at the human savings seed by accident.

## Setup checklist (all hosts)

### 1. Build or install Vaughan

```bash
cd ~/Desktop/Vaughan-CLI   # or your clone path
cargo build -p vaughan-cli -p vaughan-dapp-browser
# optional: cargo install --path vaughan-cli
# optional: cargo install --path vaughan-dapp-browser
```

Confirm: `which vaughan` or dev path `target/debug/vaughan` matches MCP config `command`.

### 2. Add MCP config on the host

**Cursor (dev, ships in repo):** [`.cursor/mcp.json`](../../../.cursor/mcp.json)

**Installed binary (any host):**

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

**VB / browser tools** — add `env` (adjust Chrome path):

```json
"env": {
  "VAUGHAN_DAPP_BROWSER_CHROME": "/usr/bin/chromium",
  "VAUGHAN_DAPP_BROWSER_CDP_PORT": "9222"
}
```

Or enable persisted CDP: `vaughan config agent-browser on` (then reconnect MCP — step 3).

Full reference: [`docs/mcp.md`](../../docs/mcp.md).

### 3. Reconnect MCP on the host (required after any config change)

MCP servers are long-lived processes. They **do not** reload config, env, or a rebuilt binary until reconnected.

| Host | Action |
|------|--------|
| **Cursor** | Settings → MCP → toggle **vaughan** off → on ([`hosts/cursor.md`](hosts/cursor.md)) |
| **Claude Desktop** | Quit app fully → reopen ([`hosts/claude-desktop.md`](hosts/claude-desktop.md)) |
| **Claude Code** | Restart session / reload MCP ([`hosts/claude-code.md`](hosts/claude-code.md)) |

### 4. Unlock the control plane (for writes + wallet-gated dApps)

Pick one:

```bash
# Interactive TUI
cargo run -p vaughan-cli
# unlock vault; leave running on PulseChain 369 or testnet 943
```

```bash
# Headless (automation / agents without a TUI window)
export VAUGHAN_WALLET_PASSWORD='…'   # never commit
vaughan serve --password-env VAUGHAN_WALLET_PASSWORD
# sentient: vaughan --profile sentient serve --password-env VAUGHAN_WALLET_PASSWORD
```

See [`docs/sentient-ops.md`](../../docs/sentient-ops.md) for `serve.env` layout.

### 5. Verify (agent runs these in order)

```
get_control_plane_status {}
get_network {}
```

**In the TUI (no terminal):** when unlocked, the **F1 network box** (top status strip) shows:

| F1 suffix | Meaning |
|-----------|---------|
| `· MCP on` | Agents can attach on loopback `:8746` |
| `· MCP on (N)` | Same + `N` proposals waiting in queue |
| `· MCP …` | Listener starting |
| `· MCP off` | Bind failed (port busy — stop `vaughan serve`, lock/unlock) |
| *(nothing)* | Wallet locked |

**Pass — MCP connected (cable 1):**

- Host lists Vaughan tools (`get_network`, `quote_swap`, `browser_*`, …)
- `get_network` → `chain_id`, `network_id`, `rpc_url`

**Pass — control plane up (cable 2):**

- `control_plane_reachable: true`
- `wallet_unlocked: true`
- `ready_for_writes: true` (needed for `propose_*`, Switch.win wallet quotes)

**Optional VB smoke:**

```
browser_status {}
browser_open { "url": "https://example.com" }
browser_snapshot {}
```

Full checklist: [`docs/mcp-smoke.md`](../../docs/mcp-smoke.md) · VB: [`docs/vb-mcp-smoke.md`](../../docs/vb-mcp-smoke.md).

## Agent browser (VB) enablement

CDP is **off by default**. Before `browser_snapshot` / `browser_open_agg`:

| Method | When |
|--------|------|
| `vaughan config agent-browser on` | Normal use; persists in `wallet.json` |
| Settings → **`p`** | Same, from TUI |
| `VAUGHAN_DAPP_BROWSER_CDP_PORT=9222` in MCP `env` | Dev/smoke override in `mcp.json` |

After any change → **reconnect MCP** (step 3). Kill-switch: [`docs/vb-kill-switch.md`](../../docs/vb-kill-switch.md).

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| No Vaughan tools in agent | Reconnect MCP; check `command` path; [`hosts/`](hosts/) |
| MCP server crashes instantly | Logs must go to **stderr** only; rebuild `vaughan-cli` |
| `wallet_locked` | Unlock TUI or run `vaughan serve` |
| `control_plane_reachable: false` | Start TUI or `serve`; check loopback not blocked |
| `browser_unavailable: agent browser control disabled` | `vaughan config agent-browser on` + reconnect MCP |
| `browser_unavailable: no vb.session` | Call `browser_open` first |
| `mainnet_blocked` | Testnet first; then `VAUGHAN_MCP_ALLOW_MAINNET=1` if intentional |
| Stale binary after `cargo build` | Reconnect MCP (Cursor re-spawns `cargo run …`) |

## Next skills

| Goal | Skill |
|------|-------|
| Ag quotes / `browser_open_agg` | [`vb-ag-quotes`](../vb-ag-quotes/SKILL.md) |
| dApp connect / inject quirks | [`dapp-connect`](../dapp-connect/SKILL.md) |
| Token addresses / PulseChain | [`pulsechain-context`](../pulsechain-context/SKILL.md) |
| Signing safety | [`core-rules`](../core-rules/SKILL.md) |

## Code / doc references

| What | Where |
|------|--------|
| MCP architecture | [`docs/mcp.md`](../../docs/mcp.md) |
| Smoke tests | [`docs/mcp-smoke.md`](../../docs/mcp-smoke.md) |
| Tool list | [`docs/ai-tool-surface.md`](../../docs/ai-tool-surface.md) |
| Project MCP config | [`.cursor/mcp.json`](../../../.cursor/mcp.json) |
| MCP server impl | `vaughan-mcp/src/server.rs`, `dispatch.rs` |
