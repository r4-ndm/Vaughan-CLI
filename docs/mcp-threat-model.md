# Vaughan MCP Threat Model (v1)

Threat model for external agents using Vaughan via MCP stdio and hybrid IPC.
Bind this document before shipping MCP v1.

## In-scope adversaries

1. **Compromised or malicious external agent** — prompt injection, wrong
   arguments, lying in `explanation` text.
2. **Other local processes** — port scan on loopback, tamper with queue files.
3. **Malicious or stale RPC** — wrong chain state, failed simulations hidden
   from the agent.

## Out of scope (explicit)

- User approves what is shown on the **adviser** (`default`) approval card.
- Outcomes on a **sentient** seed the agent (or partner) chose to fund.
- Shared-seed partnership without a skill preset (user’s choice; not Vaughan’s fault).
- OS-level keyloggers, clipboard sniffers, shoulder surfing.
- Smart contract exploits after a correct approval / policy-allowed auto-exec.

## Product model (human vs adviser vs sentient)

Full write-up: [`agent-roles.md`](agent-roles.md).

| Role | Trust assumption | Control |
|------|------------------|---------|
| **Human only** | No agent surface wanted | MCP control plane + proposal queue surfacing off entirely |
| **Adviser** (`default` / `vaughan`) | Agent may be wrong; human wants help | Every write needs human approve |
| **Sentient** (`sentient` / `vaughan-sentient`) | Agent acts on **its** seed (or shared partnership seed + skill preset) | Auto-exec under policy; don’t conflate with human `default` |

Mode is picked per profile on the unlock picker and locked for the session
(FR-5.1). Primary separation is **whose seed** and **who decides**.
Partnership = shared seed + [`sentient-presets.md`](sentient-presets.md) — no
on-chain contracts.

## Controls matrix

| Threat | Control |
|--------|---------|
| Agent lies in `explanation` | Labeled **untrusted**; calldata decode is authoritative |
| Stale simulation | Re-simulate `eth_call` at approve (default) or pre-broadcast (sentient); **except** `Batch7702` eth_call — Ambire draft uses a placeholder signature, so integrity is abi-decode `execute(txns)` + fee-spike via `estimate_self_pay_fee` + fresh `submit_batch` |
| Wrong chain | `chain_id` on proposal; reject on mismatch |
| Queue file tampering | HMAC-SHA256 over proposal bytes + session secret |
| Local socket hijack | Loopback only (`127.0.0.1:8746`); random session token in `mcp.session` (0600) |
| MCP exfiltrates keys | MCP process never unlocks vault; banned tools + tests |
| Agent spends human savings | Use distinct seeds; only share mnemonic for intentional partnership |
| Sentient batch smuggling | `Batch7702` auto-exec decodes `execute(txns)` legs; every native/ERC-20 transfer leg is sized against its own balance; unsizeable raw calls are refused (human profile required) |
| Sentient sandwich / zero min-out | Gate-time fresh quote; `min_amount_out` must be within `max_slippage_bps` of it |
| Sentient rogue router | Audited DEX router allowlist checked both at `propose_swap` and again at the auto-exec gate |
| Sentient gas drain | Fresh fee estimate (fail-closed) + pre-broadcast `check_gas_budget` against the session ceiling; fee-spike >10% rejected |
| Sentient policy downgrade | `sentient-policy.toml` written 0600, `deny_unknown_fields`, loud warning when enforcement ≠ `enforced` |
| Mainnet accident | Testnet default; `VAUGHAN_MCP_ALLOW_MAINNET=1` for mainnet writes; re-checked at the sentient gate |
| Approval flooding | Max 10 pending + 30 enqueues / 60s sliding window per profile |
| Runaway sentient agent | Profile policy + circuit breakers + **Ctrl+K** kill-switch (TUI) |
| Fee spike | Re-estimate at approve; reject when `estimated_fee_wei` set and fresh fee >10% higher (agent propose tools stamp this via core `EvmAdapter::estimate_fee`; Batch7702 uses Ambire `estimate_self_pay_fee`) |
| TOCTOU | Re-simulate + re-estimate fee before sign |
| Double-spend proposal | Terminal states; duplicate `proposal_id` rejected at enqueue |
| Queue HMAC downgrade | HMAC-SHA256 covers the `source` field too; queue dirs 0700, history 0600, reads size-capped |
| Provider token theft | Token rotates on every lock/unlock edge; `provider.session` only exists while unlocked; invalidated on exit |
| MCP listener DoS | Bind-before-token-publish, exponential backoff on bind failure, connection cap + per-connection lifetime timeout |
| Profile path traversal | Profile names validated (`[a-zA-Z0-9_-]`, ≤64) before any path join |

## VB agent control (CDP) controls

Chrome DevTools Protocol itself has **no authentication** — anything on
loopback that reaches the CDP port can drive the browser. The `cdp_token` in
`vb.session` is session metadata for agents, **not** a CDP credential. The
real controls are:

| Threat | Control |
|--------|---------|
| Well-known-port squatting (9222) | Random loopback port per spawn (`spawn_cdp_port`); env override kept for dev |
| Stale `vb.session` → foreign browser | PID binding: `vb.session` records the launcher PID; MCP verifies `/proc/<pid>` is still `vaughan-dapp-browser` before any CDP call; stale files are deleted |
| Tab hijack mid-session | Target pinning (`vb.target`): tools attach to the tab `browser_open`/`browser_navigate` opened, not "first page" |
| Agent navigates to attacker page | `data:`/`blob:` rejected as nav targets; in-tab nav gate (MV3 declarativeNetRequest) fails **closed** when `allowlist.json` is unreadable |
| Nav-gate bypass then click/type | Mutating tools re-check the **current** page URL against the session allowlist before acting (fail-closed when unreadable) |
| Snapshot leaks typed secrets | `browser_snapshot` masks input values (`hasValue` boolean only) |
| IPFS gateway wallet phishing | `browser_connect_wallet` / `browser_open_agg` auto-connect refuse on public IPFS gateway hosts — connect is a human decision there |
| Page-origin spoofing via provider | Per-launch extension secret seals `vaughan_page_origin` (AES-256-GCM, `vaughan_origin_seal`); provider rejects unsealed assertions when a key is installed |
| XPath/quote injection in click-by-text | XPath 1.0 literal quoting (`concat()` when both quote kinds present) |
| `vb.log` / `vb.target` info leak | Both written 0600 |

## Automated test coverage (approval path)

Hand-rolled MCP stdio is intentionally thin; fund-safety controls below are
what we regression-test in CI. Full `rmcp` rewrite is **not needed now** —
see [`mcp-transport.md`](mcp-transport.md).

| Threat row | Test |
|------------|------|
| Stale simulation | `vaughan-tui/tests/mcp_dogfood.rs::mcp_resim_blocks_insufficient_funds_before_sign` |
| Wrong chain | `mcp_dogfood::mcp_chain_mismatch_blocks_sign`, `vaughan-core` `apply_proposal_rejects_network_mismatch` |
| Queue tampering | `vaughan-core` `proposal_queue_rejects_tampered_hmac` |
| Local socket hijack | `vaughan-tui/tests/mcp_listener.rs::mcp_loopback_rejects_bad_session_token` |
| Mainnet accident | `vaughan-core` `guard_mainnet_write_gates` |
| Approval flooding | `proposal_queue_rejects_when_full` + `check_enqueue_rate` in enqueue |
| Fee spike at approve | `mcp_fee_spike_blocks_sign` + `fee_spike_threshold` |
| Invalid proposal_id | `validate_proposal_id_rejects_path_traversal` + `mcp_host` dispatch test |
| Duplicate proposal_id | `enqueue_rejects_duplicate_proposal_id` |
| Offline write without session | `enqueue_rejects_empty_session_secret` + dispatch `session_required` |
| IPC line size cap | `MCP_IPC_MAX_LINE_BYTES` in `decode_ipc_line` / `read_ipc_line` |
| Constant-time token compare | `session_token_valid` in `mcp_ipc` |
| Unified IPC dispatch | `vaughan-core/mcp_host.rs` (TUI + serve share one handler) |
| User reject / terminal state | `mcp_dogfood::mcp_user_reject_lands_in_rejected_queue` |
| Locked wallet | `mcp_dogfood::mcp_locked_wallet_blocks_sign` |
| Expired proposal | `mcp_dogfood::mcp_expired_proposal_blocks_sign` |
| Live IPC status without pending | `vaughan-core` `mark_approved_without_pending_still_records_status` |

Run the suite:

```sh
cargo test -p vaughan-core --lib proposal_
cargo test -p vaughan-tui --test mcp_dogfood --test mcp_listener
cargo test -p vaughan-mcp --test conformance
```

MCP stdio wire format (initialize / tools/list / tools/call envelopes): see
[`mcp-smoke.md`](mcp-smoke.md) and `vaughan-mcp/tests/conformance.rs`.

## Trust boundaries

```
┌─────────────────────────────────────────┐
│  UNTRUSTED: Cursor / agent / MCP process │
│  - Read + write tools (propose or auto)  │
│  - Never holds vault password or keys    │
└──────────────────┬──────────────────────┘
                   │ loopback socket / file queue
                   ▼
┌─────────────────────────────────────────┐
│  TRUSTED: Vaughan (unlocked profile)     │
│  - Owns WalletState + signing            │
│  - adviser (default): human approval     │
│  - sentient: policy auto-sign            │
└─────────────────────────────────────────┘
```

## v2 note

A `vaughan serve` daemon moves the trusted boundary to a long-running
unlocked process (password via `--password-env`). MCP stdio remains unprivileged.
v1 IPC types (`ProposalQueue`, session token, HMAC) become the daemon wire
protocol — no throwaway work.
