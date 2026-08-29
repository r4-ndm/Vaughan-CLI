# Cursor — Vaughan MCP

## Config file

**Project (this repo):** [`.cursor/mcp.json`](../../../../.cursor/mcp.json)

Ships two servers:

| Name | Profile | Use |
|------|---------|-----|
| `vaughan` | `default` | Adviser — human approves signs |
| `vaughan-sentient` | `sentient` | Agent-led — auto-exec under policy |

Dev config uses `cargo run -p vaughan-cli -- mcp …`. For a global install, change `command` to `"vaughan"`.

## First-time setup

1. Open the repo in **Cursor** (project root must contain `.cursor/mcp.json`).
2. Build Vaughan: `cargo build -p vaughan-cli` (and `vaughan-dapp-browser` if using VB).
3. Open **Cursor Settings** (`Ctrl+,` / `Cmd+,`).
4. Go to **Features → MCP** (label may be **Tools & MCP** in newer builds — search settings for “MCP”).
5. Confirm **`vaughan`** appears and shows **connected** (green).
6. In Agent/Composer, confirm tools like `get_network` are available (tool picker or ask the agent to call them).

Optional: enable VB in [`mcp.json`](../../../../.cursor/mcp.json) `env` or run `vaughan config agent-browser on`.

## Reconnect MCP (after config / build / agent-browser change)

This is what “restart Cursor MCP” or “toggle vaughan server” means:

1. **Cursor Settings** → **Features → MCP**
2. Find **`vaughan`** (and **`vaughan-sentient`** if you use it)
3. **Turn Off** → wait ~2 seconds → **Turn On**
4. Status should return to **connected**

**Alternative:** fully quit Cursor and reopen (same effect, slower).

You must reconnect when you change:

- `.cursor/mcp.json` (`command`, `args`, `env`)
- `vaughan config agent-browser on|off`
- A rebuilt `target/debug/vaughan` binary (when using `cargo run` in config)

## Verify

Ask the agent:

```
Run get_control_plane_status and get_network. Report both JSON results.
```

**In the Vaughan TUI:** unlocked → **F1** network box shows **`· MCP on`** when agents can connect (replaces `ss | grep 8746`). **`· MCP off`** means port 8746 was busy — stop `vaughan serve`, then lock/unlock the wallet.

| Result | Meaning |
|--------|---------|
| Tools run without “tool not found” | stdio MCP cable OK |
| `control_plane_reachable: true` | TUI or `serve` is up |
| `wallet_unlocked: true` | Writes and wallet-gated dApps OK |

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `vaughan` not in MCP list | Open repo root; check `.cursor/mcp.json` exists |
| Red / disconnected | Click refresh; check `cargo build` succeeds; read MCP log in Settings |
| Tools missing `browser_*` | Add CDP env or `vaughan config agent-browser on`; reconnect |
| Wrong profile | Edit `"args": ["mcp", "--profile", "default"]` |

Smoke checklist: [`docs/mcp-smoke.md`](../../../../docs/mcp-smoke.md).
