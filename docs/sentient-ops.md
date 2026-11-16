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

### Sentient spend guard & UID isolation
Sentient mode bounds money movement across all autonomous sinks:
- **Spend caps**: Policies define per-trade and daily caps per asset. The ledger file is stored inside the signer daemon store (`/var/lib/vaughan/sentient/sentient-spend.json`, 0600, owned by `vaughan-signer`), surviving restarts. Failed broadcasts still consume reserved budget.
- **Allowlist & typed intents**: `allow_transfer_to` defaults empty (refuses transfers outside allowlist). Calldata is constructed inside the signer daemon from typed intents only; untyped calldata is refused without touching the signer.
- **UID boundary**: The signer daemon runs as dedicated system user `vaughan-signer`. External processes and agents connect via `/run/vaughan/signerd.sock` subject to peer UID authentication (`SO_PEERCRED`), and cannot tamper with policies, spend ledgers, or keys.

1. Create / fund profile: `vaughan --profile sentient create` (or restore).
2. Apply a preset: `vaughan --profile sentient preset apply balanced`.
3. Install and start the isolated signer daemon:

```bash
sudo bash scripts/install-signerd.sh
# Verifies /var/lib/vaughan/sentient ownership, migrates files, and enables vaughan-signerd.service
```

4. Run the thin serve client (or let MCP connect directly over `/run/vaughan/signerd.sock`):

```bash
vaughan --profile sentient serve
```

5. Point MCP at that profile (`vaughan-sentient` in Cursor — see [`mcp.md`](mcp.md)).

6. Inspect or update live sentient policy through `vaughan-signerd` (no manual root edits to `/var/lib/vaughan/sentient`):

```bash
vaughan --profile sentient policy show
vaughan --profile sentient policy set --daily-cap-pct 50 --max-slippage-bps 250
# Enabling enforcement=disabled requires a two-step confirmation:
vaughan --profile sentient policy confirm-unsafe
vaughan --profile sentient policy set --enforcement disabled
```

Policy changes use out-of-band root approval: the CLI/TUI sends `{"op":"policy_update","auth":"stage"}` (client `policy_stage`), which writes `policy-pending.json` (0600, 10-min TTL) in the signer store; an operator runs `/usr/local/bin/vaughan signerd approve` as root (from a root login on a clean console (`Ctrl+Alt+F3`), `run0`, or `sudo` from a separate admin account — never from a shell owned by the agent UID), reviews the diff on an interactive TTY, and confirms, writing one-time `policy-approval.json` (0440, `root:vaughan-signer` or the store's group if `vaughan-signer` does not exist); the client polls `{"op":"policy_pending"}` and then sends `{"op":"policy_update","auth":"approved"}`. The daemon fstat-verifies (`uid == 0`, `nlink == 1`, `mode == 0440`) and unlinks the approval before applying, reloads the active `CircuitBreaker`, and logs `{ts, field_changes, source}` to `policy-audit.jsonl` (never any password). Press `y` in the TUI **Settings** view on a sentient profile to edit and stage a change over `/run/vaughan/signerd.sock`.

7. Agent loop pattern:

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
Kill-switch: **Ctrl+K** in the TUI trips the session breaker **and** sends the
daemon's `trip` op (below), so the thing that holds the keys stops too and stays
stopped across a restart. The flash is explicit about what happened — only
"daemon HALTED" means the daemon stopped; "daemon NOT reached … local breaker
only", "daemon too old — NOT halted" and "daemon HALTED but NOT on disk" each
mean you still have work to do (stop the unit). Ctrl+K stays a no-op on a
non-sentient profile: the socket is machine-global and a human wallet never
asked the daemon to sign. The human `default` profile never auto-signs.

## Gas ceiling: set it before you upgrade the daemon

From 0.2.12 the daemon **meters every broadcast** against `max_session_gas_wei`:
it prices the transaction, reserves `gas_limit × max_fee_per_gas` against the
ceiling, and refuses (`gas_ceiling_exceeded`) rather than signing when the quote
does not fit. A transaction it cannot price is refused too (`gas_unavailable`) —
nothing is ever treated as free.

**The stock default (5e16 wei) refuses EVERY transaction on PulseChain.**
Measured `eth_gasPrice` on 2026-11-16: chain 369 ≈ 1.9e15 wei/gas, chain 943 ≈
2.4e13 wei/gas, so a bare 21,000-gas transfer costs ~4e19 wei on 369 — about
800× the default ceiling. Raise it **before** upgrading the host daemon, through
the normal root-approved flow (the field already exists; the old daemon ignores
it):

```bash
vaughan --profile sentient policy set --max-session-gas-wei 5000000000000000000000   # 5e21 = 5,000 PLS, chain 369
# chain 943 (testnet): 5e20 is a reasonable starting point
```

Pick the number deliberately and adjust to the real fee level. Two footguns, both
unvalidated: `U256::MAX` effectively disables the ceiling, and `0` refuses
everything. After the upgrade, the daemon's `status` op — which the MCP
`budget_status` tool returns verbatim — reports what is in force:

```
budget.gas_spent_wei / budget.gas_max_wei / budget.gas_remaining_wei
```

A DCA plan whose slice the daemon refuses on gas (`gas_ceiling_exceeded`,
`gas_unavailable`, `circuit_breaker_tripped`) is **deferred, not failed**: nothing
was spent, so the slice does not count toward the three-failure pause and does
not burn the buy window. The slice log shows `deferred (not counted): …` and the
plan retries about a minute later, so a too-tight ceiling shows up as a stalled
plan, not a paused one. A `broadcast_uncertain` slice still counts — it may have
executed.

The counter is per daemon process and in memory: restarting the daemon refunds
the whole gas budget. On Base/Arbitrum the ceiling under-counts (unpriced L1 data
fee) — see [`mcp-threat-model.md`](mcp-threat-model.md). Never try to change
policy from an agent; it needs root approval by design.

## Halting the daemon (durable kill switch)

Any peer the socket allowlist admits may halt the daemon:

```bash
# one newline-delimited JSON line; any tool that speaks a Unix socket works
printf '{"op":"trip","reason":"operator stop: investigating"}\n' \
  | socat - UNIX-CONNECT:/run/vaughan/signerd.sock   # or: nc -U /run/vaughan/signerd.sock
```

Every later `sign` is refused with `circuit_breaker_tripped`, and the reason is
written to `/var/lib/vaughan/sentient/signerd-tripped.json` (0600) **before** the
op returns ok. If that write fails the answer is `ok:false` /
`trip_not_persisted`: the daemon is halted in memory only, so stop the unit
instead of trusting it. Automatic trips (gas ceiling, consecutive-error
tripwire) are persisted the same way, and the trip is appended to
`policy-audit.jsonl` with the peer UID.

The marker is loaded before the socket is bound and **fails closed**: a marker
that cannot be read, parsed, or trusted (wrong mode/owner) also starts the
daemon halted. That is what makes `Restart=on-failure` safe — a panic or OOM kill
would otherwise bring the breaker back clean within seconds.

**There is no untrip op.** To resume trading, as root:

```bash
systemctl stop vaughan-signerd
cat /var/lib/vaughan/sentient/signerd-tripped.json   # read WHY before clearing it
rm /var/lib/vaughan/sentient/signerd-tripped.json
systemctl start vaughan-signerd
```

If a `broadcast_uncertain` error is what halted things, check the chain for the
transaction before clearing: that code means the raw transaction may already have
been accepted, and the spend-ledger leg is deliberately kept so an identical
retry answers `spend_already_reserved` instead of sending twice.

Vaughan does **not** run a general background price daemon. The agent (or a cron
that calls MCP) owns ad-hoc poll intervals. Exception: **TUI-hosted DCA** — while
a Sentient session is unlocked, the App tick fires due `dca-plans.json` slices
via aggregators under circuit breakers (see [`dca.md`](dca.md)). Closing the TUI
pauses DCA.

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

## Why Sentient works better with David’s patterns

[DavidFeder/pulsechain-mcp](https://github.com/DavidFeder/pulsechain-mcp) is a
battle-tested PulseChain agent playbook. Vaughan does **not** vendor that
TypeScript — we reimplement the *habits* in Rust (`vaughan-core` + MCP tools).
Sentient (agent-led auto-exec) benefits most because it must decide without a
human on every click.

| Habit (David-style) | What Vaughan does | Why Sentient cares |
|---------------------|-------------------|--------------------|
| **See the market** | DexScreener research tools — no API key ([`dexscreener.md`](dexscreener.md)) | Finds pairs/prices instead of guessing |
| **Address beats ticker** | Spoof-aware search + e*/p* origin labels on resolve/list | Avoids fake tickers and eth-vs-pulse mixups |
| **Fail soft** | Soft-fail JSON on network/429 — not a hard MCP crash | Agent can retry or stop cleanly mid-loop |
| **Safe writes** | pHEX-only stake proposes; unlimited approve refused at the gate ([`hex-stake.md`](hex-stake.md), auto-exec above) | Circuit breakers alone are not enough if the *proposal* is already wrong |
| **Readable review** | Proposal verify rows for Advisor cards (and Sentient logs the same facts) | Humans (and you debugging) can see what would run |

**Analogy:** before = smart intern, no map, blank check. After = same intern with
a PulseChain street map, a list of which doors are real, and a rule that says
never sign blank checks — while circuit breakers still cap size/gas/slippage.

## Related

- [`mcp.md`](mcp.md) — setup  
- [`mcp-transport.md`](mcp-transport.md) — hand-rolled vs `rmcp`  
- [`agent-roles.md`](agent-roles.md) — adviser vs sentient  
- [`dexscreener.md`](dexscreener.md) / [`hex-stake.md`](hex-stake.md) — David-inspired tool detail  
- [`mcp-threat-model.md`](mcp-threat-model.md) — hot-wallet warning for serve  
- [`sentient-presets.md`](sentient-presets.md) — skill packs  
