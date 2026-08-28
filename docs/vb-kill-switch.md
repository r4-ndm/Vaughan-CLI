# Vaughan Browser (VB) kill-switch

Operator runbook for disabling the optional Chromium dApp browser side door
without breaking **Vaughan Wiz4rd-Engine** (Ag / Dex / Browse / MCP wallet tools)
or the EIP-1193 provider.

## Default safe posture

- **Agent browser control (CDP): OFF** — persisted in `wallet.json` per profile
- TUI `w` and MCP `browser_open` launch VB **without** loopback CDP unless enabled
- Signing never auto-approves — all sign/send still hits the Vaughan TUI

Enable CDP only when you need MCP `browser_*` navigation:

| Path | Command |
|------|---------|
| TUI | Unlock → Settings (`n`) → **`p`** |
| CLI | `vaughan config agent-browser on` |
| MCP host env | `VAUGHAN_DAPP_BROWSER_CDP_PORT=9222` in `mcp.json` (dev/smoke override) |

Precedence: **env var > persisted toggle > off**.

## Soft disable (recommended)

1. `vaughan config agent-browser off` (or Settings → **`p`**)
2. Close any open **Vaughan Browser** window
3. Optional: remove `vaughan-dapp-browser` from `PATH`

**Still works:** vault, send, Ag, Dex, contract browser, MCP read/write propose tools,
`vaughan-provider`, `vaughan serve`.

**Degrades:** MCP `browser_navigate` / `browser_snapshot` / click / type return
`browser_unavailable`. MCP `browser_open` may still open VB for human browsing (no CDP).

## Hard disable (developers)

- Omit `vaughan-dapp-browser` from workspace builds or uninstall the binary
- `vaughan-core`, `vaughan-provider`, and default `vaughan-cli` **do not** link CEF/Chromium

## Emergency

1. Kill the VB Chromium child process
2. Delete `~/.local/share/vaughan-cli/vb.session` (or `$XDG_DATA_HOME/vaughan-cli/vb.session`)
3. `vaughan config agent-browser off`

Note: a running Chromium process may keep CDP port open until the window closes —
MCP tools re-check the toggle on each call, but loopback CDP remains reachable until exit.

## What this does not disable

- Browserless Pulse flows (no web engine)
- Freedom Browser integration code (parked until upstream PR #195)
- Provider WebSocket on `127.0.0.1:8745` (separate from CDP)

See also: [dapp-browser-strategy.md](dapp-browser-strategy.md), [mcp.md](mcp.md),
[Security-Table.md](Security-Table.md).
