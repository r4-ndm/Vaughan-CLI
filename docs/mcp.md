# Vaughan MCP Setup

Vaughan exposes a **Model Context Protocol (MCP)** stdio server so external
agents (Cursor, Claude Code, Codex) can use the same DeFi verbs as the TUI.

**Two roles** — details: [`agent-roles.md`](agent-roles.md):

1. **Adviser** — you use Vaughan; agent proposes; you approve (`vaughan` /
   `--profile default`).
2. **Sentient** — the agent uses **its own seed** and acts with full control
   (`vaughan-sentient` / `--profile sentient`). That seed lives in `vaughan
   signerd`; MCP intents auto-exec there under policy (re-sim + caps), no
   approval card. Human may **partner** by sharing that same seed + a skill
   preset.

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

Separate MCP entry on the **agent’s** profile. Nothing to unlock for it: those
keys live in `vaughan signerd`, and `vaughan mcp` submits typed intents over
`/run/vaughan/signerd.sock` (see [Architecture](#architecture)).

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

## Architecture

- **Keys stay in signer daemon / Vaughan** — MCP never unlocks or holds vault keys.
- **Adviser path:** MCP → loopback TCP (`127.0.0.1:8746`) → TUI approval card → human approval.
- **Sentient path:** MCP → Unix domain socket (`/run/vaughan/signerd.sock`, peer UID authenticated) → isolated `vaughan-signerd` daemon (policy, breaker, typed calldata construction) → broadcast. Untyped tools return `intent_untyped` naming the adviser path. No `mcp.session` token file exists for sentient.
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
| `wallet_locked` (adviser) | Unlock Vaughan TUI; or pass explicit `account_address` on read tools |
| `pending_user` forever (adviser) | TUI must be open and unlocked for live socket path; sentient intents never queue for approval |
| `mainnet_blocked` | Sentient: set `VAUGHAN_MCP_ALLOW_MAINNET=1` in the **daemon's own** root-owned env (`/etc/vaughan/sentient.env`) and restart it — a client-side variable does nothing. Adviser: same variable in the serve/TUI process. Use testnet first |
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
# Sentient — thin client of vaughan-signerd (no password, no token, no listener)
vaughan --profile sentient serve

# Adviser — unlocks the vault, writes the loopback session token
export VAUGHAN_WALLET_PASSWORD='…'   # never commit; use a secret manager
vaughan serve --password-env VAUGHAN_WALLET_PASSWORD
```

**Adviser (`default`) — unlocks the vault**

- Unlocks the profile vault non-interactively, writes the session token
  (`mcp.session`, 0600), binds loopback MCP control (`127.0.0.1:8746`).
- Reads the password from `--password-env` once, then **unsets** that env var in
  the process (still prefer a short-lived secret injection; do not leave passwords
  in shell history or shared hosts).
- Queues `pending_user` for normal proposes — **never auto-signs**. Stealth
  **sweep** still needs an unlocked TUI on adviser (serve returns `tui_required`).
- Ctrl-C stops the daemon and invalidates the session token.

**Sentient (`sentient`) — thin client of the signer daemon**

- Since v0.2.x a sentient profile's keys live in `vaughan signerd` (systemd
  service, user `vaughan-signer`, socket `/run/vaughan/signerd.sock`, peer-UID
  allowlist). `vaughan serve` here does **not** unlock the vault, holds no keys,
  writes no `mcp.session`, opens no TCP port, and takes no password. It attaches
  to the socket; `vaughan mcp` for the same profile submits typed intents there.
- Auto-exec under policy (same as unlocked TUI). Spend is bounded by the
  `sentient-spend.json` ledger in the signer store
  (`/var/lib/vaughan/sentient`) plus the policy caps (`daily_cap_pct`,
  `max_position_pct`, `max_approve_pct`) sized from the account's **live
  on-chain balance**, read fresh per request. Sized approve/unwrap legs are
  gated like transfers; **unlimited** (`type(uint256).max`) approve is always
  refused for Sentient auto-exec (use a sized amount or the Advisor profile).
  Revoke (`approve(…, 0)`) is OK.
- Policy changes stage via `{"op":"policy_update","auth":"stage"}` (client
  `policy_stage`) and require out-of-band root approval
  (`sudo /usr/local/bin/vaughan signerd approve`) before
  `{"op":"policy_update","auth":"approved"}` applies them. No vault password
  is involved anywhere in that flow — the password path was removed in v0.2.10;
  check a staged change with `vaughan policy pending`. See
  [`sentient-ops.md`](sentient-ops.md).
- Residual risk (stated honestly): a same-UID process can still edit the agent's
  profile files (proposals, DCA plans, config), but signing keys, policy and the
  spend ledger sit under the `vaughan-signer` uid and are only reachable through
  daemon policy.
- Ctrl-C stops this client; the daemon keeps running.

Both profiles:

- Mainnet writes are refused (`mainnet_blocked`) unless the process that signs
  has `VAUGHAN_MCP_ALLOW_MAINNET=1` in its **own** environment: the signer
  daemon's root-owned `/etc/vaughan/sentient.env` for sentient, the serve/TUI
  process for adviser. Setting it on the MCP client changes nothing.
- EmpX quotes/proposes are PulseChain **mainnet (369) only**.
- MCP stdio (`vaughan mcp`) remains the agent-facing process; adviser attaches to
  the serve/TUI loopback socket, sentient to `vaughan-signerd`.
