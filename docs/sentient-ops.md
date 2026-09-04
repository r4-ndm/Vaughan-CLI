# Sentient ops: always-on control plane (not cloud multi-tenant)

Vaughan is **one machine, one OS user, N profiles**. That is intentional — not a
hosted SaaS wallet. This doc covers making a **sentient** (or adviser) agent
reliable without pretending we are AWS.

## What “always-on” means here

| Mode | Process | Writes |
|------|---------|--------|
| TUI unlocked | `vaughan` interactive | Adviser: approval cards · Sentient: auto-exec |
| Headless | `vaughan serve` | Same IPC; sentient auto-exec under policy |
| Neither | — | Reads may work via RPC; **writes fail** (`tui_offline` / `session_required`) |

There is **no fire-and-forget cloud agent**. Signing always lives in a Vaughan
process you control. Agents should call `get_control_plane_status` before
propose loops.

## Switching agent mode (TUI)

The unlock screen doubles as the mode switch (FR-5.1: mode locks at unlock,
never mid-session). The picker asks **Human or Sentient** first — whose seed
backs the session:

- **Human — your wallet**: then pick the mode — **Human only** (no MCP
  control plane, no proposal queue, no agent surface at all) or **Advisor**
  (agent proposes via MCP, you approve every write). With several human
  wallets on disk, a wallet list comes first.
- **Sentient — agent wallet**: always auto-exec under policy; the mode step
  is skipped because auto-exec never runs on a human wallet's seed.

Details:

- `vaughan --profile sentient` launches straight into the sentient vault.
- Advisor is pre-selected for human wallets; Sentient shows
  `(new — vault created next)` until the agent wallet exists.
- From inside the wallet: `l` locks → picker → Human/Sentient → password.
- The sentient password screen shows the live policy bounds (enforcement,
  max %/trade, slippage cap) **before** you unlock; the F1 strip shows
  `· Sentient` / `· Human` while a session is active.

## Recommended sentient stack

1. Create / fund profile: `vaughan --profile sentient create` (or restore).
2. Apply a preset: `vaughan --profile sentient preset apply balanced`.
3. Put the unlock password in a **0600** env file (never commit it):

```bash
mkdir -p ~/.config/vaughan
umask 077
printf 'VAUGHAN_WALLET_PASSWORD=…\n' > ~/.config/vaughan/serve.env
chmod 600 ~/.config/vaughan/serve.env
```

4. Run the control plane:

```bash
vaughan --profile sentient serve --password-env VAUGHAN_WALLET_PASSWORD
```

Or install the example user unit: [`scripts/vaughan-serve.service`](../scripts/vaughan-serve.service).

5. Point MCP at that profile (`vaughan-sentient` in Cursor — see [`mcp.md`](mcp.md)).

6. Agent loop pattern:

```
get_control_plane_status  →  require ready_for_writes
watch_balance / watch_quote  →  if alert
propose_*  →  auto-exec (sentient) or pending_user (adviser)
get_proposal_status
```

## Watch / trigger building blocks

| Tool | Role |
|------|------|
| `watch_balance` | Native/ERC-20 snapshot + min/max wei flags |
| `watch_quote` | Aggregator quote snapshot + min/max out flags + `suggested_action` |
| Circuit breakers / `sentient-policy.toml` | Hard stops on size, gas, slippage (sentient) |

Auto-exec additionally re-checks at the gate: per-leg sizing for transfers,
swaps, sized approve/unwrap (unlimited approve refused), and `Batch7702`
(unsizeable raw `contract_call` legs are refused), fresh quote + slippage floor
on DEX routers (agg routers need `min_amount_out > 0`), audited DEX/Agg
allowlist, fresh fee estimate (fail-closed) + pre-broadcast gas-budget check,
and the mainnet-write guard.
Kill-switch: **Ctrl+K** in the TUI trips the session breaker. The human
`default` profile never auto-signs.

Vaughan does **not** run a background price daemon. The agent (or a cron that
calls MCP) owns the poll interval.

## Multi-tenant / multi-user

**Out of scope by design:**

- Shared host with untrusted other users
- Multi-tenant cloud control plane
- Remote signing over the public internet

**In scope:**

- Multiple **profiles** under one user (`default`, `sentient`, custom names)
- Distinct seeds per profile (adviser savings ≠ sentient capital)
- Loopback IPC + session token (same OS user only)

If you need isolation between people, use separate OS users / machines — not
one Vaughan instance.

## MCP transport maintainability

Tool **schemas** for DeFi verbs come from `vaughan-agent` registries (not a
hand-maintained duplicate list). Session bridge tools
(`get_address`, `get_control_plane_status`, …) stay thin in `vaughan-mcp`.

**Conformance:** `cargo test -p vaughan-mcp --test conformance` + human checklist
[`mcp-smoke.md`](mcp-smoke.md).

**`rmcp`:** not needed now. Decision, revisit triggers, and a future spike recipe:
[`mcp-transport.md`](mcp-transport.md).

## Related

- [`mcp.md`](mcp.md) — setup  
- [`mcp-transport.md`](mcp-transport.md) — hand-rolled vs `rmcp`  
- [`agent-roles.md`](agent-roles.md) — adviser vs sentient  
- [`mcp-threat-model.md`](mcp-threat-model.md) — hot-wallet warning for serve  
- [`sentient-presets.md`](sentient-presets.md) — skill packs  
