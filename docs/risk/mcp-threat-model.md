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

Full write-up: [`agent-roles.md`](../agents/agent-roles.md).

| Role | Trust assumption | Control |
|------|------------------|---------|
| **Human only** | No agent surface wanted | MCP control plane + proposal queue surfacing off entirely |
| **Adviser** (`default` / `vaughan`) | Agent may be wrong; human wants help | Every write needs human approve |
| **Sentient** (`sentient` / `vaughan-sentient`) | Agent acts on **its** seed (or shared partnership seed + skill preset) | Auto-exec under policy; don’t conflate with human `default` |

Mode is picked per profile on the unlock picker and locked for the session
(FR-5.1). Primary separation is **whose seed** and **who decides**.
Partnership = shared seed + [`sentient-presets.md`](../agents/sentient-presets.md) — no
on-chain contracts.

## Controls matrix

| Threat | Control |
|--------|---------|
| Agent lies in `explanation` | Labeled **untrusted**; calldata decode is authoritative |
| Stale simulation | Re-simulate `eth_call` at approve (default) or pre-broadcast (sentient); **except** `Batch7702` eth_call — Ambire draft uses a placeholder signature, so integrity is abi-decode `execute(txns)` + fee-spike via `estimate_self_pay_fee` + fresh `submit_batch` |
| Wrong chain | `chain_id` on proposal; reject on mismatch |
| Queue file tampering | HMAC-SHA256 over proposal bytes + session secret |
| Local socket hijack | Adviser: loopback TCP (`127.0.0.1:8746`) with random session token in `mcp.session` (0600) · Sentient: Unix domain socket (`/run/vaughan/signerd.sock`) protected by `SO_PEERCRED` UID allowlist (`vaughan-signerd` daemon under `vaughan-signer` user); no TCP port or `mcp.session` file exists for sentient |
| MCP exfiltrates keys | MCP process never unlocks vault; banned tools + tests |
| Agent spends human savings | Use distinct seeds; only share mnemonic for intentional partnership |
| Sentient batch smuggling | The signer daemon takes typed `SignerIntent`s only (no calldata member, `deny_unknown_fields`); every `BatchTransfer` leg is checked against `allow_transfer_to` and the whole batch is reserved against the daily spend ledger; unrecognised calls never become an intent (`intent_untyped`) |
| Sentient sandwich / zero min-out | The daemon quotes on-chain itself and clamps `min_amount_out` to `max_slippage_bps` of that quote (`vaughan-cli/src/signerd/ops/swap.rs`); calldata is built by the daemon, never accepted from the agent |
| Sentient rogue router | Audited DEX router allowlist at `propose_swap` and again in the daemon (`router_not_allowed`, `spender_not_allowed`; covered by `signer_intent` tests) |
| Sentient gas drain | Enforced in the daemon: every metered broadcast is priced first (`quote_fee`), its worst-case fee (`gas_limit × max_fee_per_gas`) is RESERVED against `max_session_gas_wei` before signing, and an over-budget quote is refused (`gas_ceiling_exceeded`) without sending. A tx that cannot be priced is refused (`gas_unavailable`), never treated as free. See "Sentient gas ceiling" below for its four documented limits |
| Sentient policy downgrade | `sentient-policy.toml` written 0600, `deny_unknown_fields`, loud warning when enforcement ≠ `enforced`; live edits require root-approved `policy_update` (`stage` -> `sudo vaughan signerd approve` -> `approved`), `expected_digest` CAS, two-step `acknowledge_unsafe` before `disabled`, per-UID rate limiting, and `policy-audit.jsonl` logging |
| Mainnet accident | Testnet default; the signer daemon refuses any intent on a chain that is not a known testnet (`mainnet_blocked`; an unknown chain counts as mainnet) unless its OWN environment sets `VAUGHAN_MCP_ALLOW_MAINNET=1` (root-owned `/etc/vaughan/sentient.env`, read at startup, never settable by a request); it also refuses any intent whose `chain_id` differs from its configured chain (`chain_id_mismatch`) |
| Approval flooding | Max 10 pending + 30 enqueues / 60s sliding window per profile |
| Runaway sentient agent | Daily spend cap + position caps + session gas ceiling + circuit breaker inside the daemon. The daemon has a kill switch: `{"op":"trip","reason":"…"}` from any allowlisted peer halts it, persists the reason to `signerd-tripped.json`, and survives a restart (see "Durable kill switch" below). There is no untrip op. **Ctrl+K in the TUI sends that `trip` op** (`vaughan-tui/src/killswitch.rs`) in addition to tripping the TUI-side breaker, and its flash states which of the two actually halted: a daemon that is unreachable, too old for the op, or unable to persist the marker is reported as NOT halted / not durable rather than as a successful kill. Every cap is sized from the account's live on-chain balance (`vaughan-cli/src/signerd/balance.rs::BalanceSource`, read fresh per request); a balance read failure refuses the intent (`balance_unavailable`) rather than falling back to a default or cached value |
| Fee spike | Re-estimate at approve; reject when `estimated_fee_wei` set and fresh fee >10% higher (agent propose tools stamp this via core `EvmAdapter::estimate_fee`; Batch7702 uses Ambire `estimate_self_pay_fee`; **includes** `LpDeployStep` multi-step Brew) |
| TOCTOU | Re-simulate + re-estimate fee before sign |
| Double-spend proposal | Terminal states; duplicate `proposal_id` rejected at enqueue |
| Queue HMAC downgrade | HMAC-SHA256 covers the `source` field too; queue dirs 0700, history 0600, reads size-capped |
| Provider token theft | Token rotates on every lock/unlock edge; `provider.session` only exists while unlocked; invalidated on exit |
| MCP listener DoS | Bind-before-token-publish, exponential backoff on bind failure, connection cap + per-connection lifetime timeout |
| Profile path traversal | Profile names validated (`[a-zA-Z0-9_-]`, ≤64) before any path join |

## Sentient gas ceiling (what it bounds, and what it does not)

Every broadcast the daemon makes for a typed intent (transfer, swap, approve, wrap, unwrap,
batch transfer, DCA slice) goes through one metered path
(`vaughan-cli/src/signerd/gas.rs::broadcast_metered`): quote the fee, reserve it, re-check the
trip, send the **quoted** tx verbatim. The quoted transaction is byte-for-byte the signed
transaction (`WalletState::fill_fee` is shared by both paths); a quote is refused unless
`gas_limit >= 21_000` and `max_fee_per_gas > 0`. There is deliberately **no upper bound on the
gas price**: PulseChain 369 legitimately quotes ~2e15 wei/gas, so a "1000 gwei" style cut-off
would refuse every transaction there.

Error codes an agent sees, and how it should react:

| Code | Meaning | Retry class |
|------|---------|-------------|
| `gas_ceiling_exceeded` | The quote does not fit the remaining `max_session_gas_wei`. The message names the key, the ceiling, spent, remaining, and this quote | Shrink the tx or wait — do not hammer |
| `gas_unavailable` | The fee could not be priced or failed validation (RPC down, `estimate_gas` revert, zero/absent fee). A reverting estimate is a refusal: the tx would likely revert and burn gas | Back off |
| `broadcast_uncertain` | The raw tx may already be in a mempool (transport error/timeout after signing). The spend-ledger legs and the gas reservation are KEPT, so an identical retry answers `spend_already_reserved` | STOP; an operator must look |
| `circuit_breaker_tripped` | Trading is halted (kill switch, gas ceiling via `record_success`, or the consecutive-error tripwire) | Stop |
| `policy_load_failed` | No policy file in the store, or one that cannot be read or turned into a breaker config. A MISSING file is a refusal, not the defaults: `load_policy` answers `Ok(default)` for absence, and the built-in defaults can be far looser than the ceilings the daemon was started under. Nothing is signed | Stop; restore the policy file or stage one via `policy_update` |

`vaughan_core::core::signer_client::{error_code, retry_class}` map a returned
`"{code}: {message}"` string to these classes so callers stop string-matching ad hoc.

Four limits are real and deliberate:

1. **The counter is per process and in memory.** The daemon and the (dormant) in-TUI
   `SentientTrader` are separate `CircuitBreaker` instances, and a daemon restart refunds the
   whole gas budget. Persisting the counter is out of scope; the trip file is not.
   The in-process `SentientTrader` keeps its own *retrospective* ceiling, which is fail-open by
   one transaction (it books `gasUsed` after the send).
2. **On known L2s the ceiling under-counts.** Base (8453) and Arbitrum One (42161) add an L1
   data fee that `estimate_fee` does not price, so the real cost can exceed
   `max_session_gas_wei`. The daemon logs a warning at startup on those chains. Pricing the L1
   component is out of scope.
3. **The stealth sweep is not metered.** Its fee is paid out of the swept note itself
   (`vaughan-core/src/core/stealth.rs` refuses when `balance <= gas`), so it is the one
   broadcast outside the metered path; a source-guard test keeps every other op inside it. The
   sweep also no longer calls `record_success`, so an agent cannot clear a building failure
   streak with unmetered sweeps.
4. **An uncertain broadcast over-counts.** Keeping the reservation (and the ledger legs) for a
   send that may have reached the node is the conservative error: a genuinely lost tx costs one
   worst-case fee of head-room until the daemon restarts, rather than letting a tx that DID
   execute be re-armed by an identical retry.

A trip cannot recall an envelope already handed to the RPC. The trip is re-checked immediately
before the send, so the window is one sign+send round trip.

## Durable kill switch (`{"op":"trip"}`)

Any peer the `SO_PEERCRED` allowlist already admits may trip the daemon: stopping is the safe
direction and needs no root step-up. There is **no untrip op** over the socket.

- The reason is stripped of control characters and clamped to 200 chars/bytes before it is
  formatted into a message, a log, or a file (it reaches an operator's terminal).
- First reason wins, in memory and on disk: an automatic trip racing a manual kill switch
  cannot have its reason clobbered by whichever call lands second.
- The trip is persisted to `<store>/signerd-tripped.json` (mode `0600`, temp file + `fsync` +
  rename) BEFORE the op returns. If that write fails the daemon is still halted in memory but
  answers `ok:false` / `trip_not_persisted` — a restart would come back untripped, so the
  operator must stop the unit.
- Automatic trips (gas ceiling, consecutive-error tripwire) are persisted after every `sign`
  for the same reason.
- `Signer::new` loads the marker BEFORE the socket is bound, and fails **closed**: a marker
  that exists but cannot be read, parsed, or trusted (wrong mode/owner) also starts the daemon
  tripped. Only a genuinely absent marker means "not halted". This matters because
  `Restart=on-failure` plus workspace `panic = "abort"` means any panic or OOM kill would
  otherwise bring the daemon back with a clean breaker in a few seconds.
- **Clearing a trip is root removing `/var/lib/vaughan/sentient/signerd-tripped.json` and
  restarting the unit** (`systemctl restart vaughan-signerd`). See
  [`sentient-ops.md`](../agents/sentient-ops.md).
- Every trip appends `{ts, event, peer_uid, reason}` to `policy-audit.jsonl` (`peer_uid` is
  `null` for an automatic trip): a kill switch leaves a trace next to the policy changes it
  overrides.

`status` reports `budget.gas_spent_wei`, `budget.gas_max_wei`, `budget.gas_remaining_wei` from
the config actually in force, alongside `budget.circuit_breaker_tripped` / `_reason`. The
policy is re-read per `sign` request and reapplied to the breaker, so a ceiling tightened on
disk binds the next request rather than the next restart.

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
see [`mcp-transport.md`](../architecture/mcp-transport.md).

| Threat row | Test |
|------------|------|
| Stale simulation | `vaughan-tui/tests/mcp_dogfood.rs::mcp_resim_blocks_insufficient_funds_before_sign` |
| Wrong chain | `mcp_dogfood::mcp_chain_mismatch_blocks_sign`, `vaughan-core` `apply_proposal_rejects_network_mismatch` |
| Queue tampering | `vaughan-core` `proposal_queue_rejects_tampered_hmac` |
| Local socket hijack | `vaughan-tui/tests/mcp_listener.rs::mcp_loopback_rejects_bad_session_token` |
| Sentient signer peer UID rejection | `vaughan-cli/tests/signer_intent.rs::test_socket_peer_uid_rejection` |
| Sentient policy root-approval step-up & audit | `vaughan-cli/tests/signer_intent.rs::test_policy_update_*`, `test_s3b_*`, `test_s3c4_*` |
| Sentient untyped tool isolation | `vaughan-mcp/tests/sentient_cutover.rs::test_untyped_tools_return_intent_untyped_and_do_not_touch_signer` |
| Sentient signer policy enforcement | `vaughan-mcp/tests/sentient_cutover.rs::test_signer_refusal_surfaces_unchanged_through_mcp` |
| Sentient no `mcp.session` written | `vaughan-cli/tests/serve_sentient_gate.rs::test_serve_sentient_creates_no_mcp_session_file` |
| Mainnet accident | `vaughan-core` `guard_mainnet_write_gates` |
| Sentient gas drain | `vaughan-cli/tests/signerd_gas.rs` (ceiling refusal, cumulative/boundary admission, atomic concurrent reservations, quote-validation floors, saturation, every op kind metered, source guard that no op but `sweep.rs` bypasses metering) |
| Quoted fee == signed fee | `vaughan-cli/tests/signerd_fee_anvil.rs::test_quoted_fee_is_the_fee_the_node_sees` (anvil) |
| Uncertain broadcast keeps the reservation | `signerd_gas.rs::test_uncertain_broadcast_failure_keeps_gas_and_legs` |
| Durable kill switch | `vaughan-cli/tests/signerd_trip.rs` (signs refused after `trip`, first reason wins in memory and on disk, reason sanitised, survives a restart, file `0600`, corrupt marker starts tripped, automatic trip persisted, audit line written) |
| Policy freshness per request | `signerd_trip.rs::test_policy_edit_on_disk_tightens_the_next_sign` |
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
[`mcp-smoke.md`](../ops/mcp-smoke.md) and `vaughan-mcp/tests/conformance.rs`.

## Trust boundaries

```
┌─────────────────────────────────────────┐
│  UNTRUSTED: Cursor / agent / MCP process │
│  - Runs as host UID (e.g. uid 1000)     │
│  - Communicates via Unix domain socket  │
│  - Typed intents only (untyped refused) │
│  - Never holds keys, passwords, or caps │
└──────────────────┬──────────────────────┘
                   │ Unix socket (/run/vaughan/signerd.sock, SO_PEERCRED)
                   ▼
┌─────────────────────────────────────────┐
│  TRUSTED: vaughan-signerd daemon        │
│  - Runs as dedicated vaughan-signer UID │
│  - Owns store at /var/lib/vaughan/      │
│  - Enforces policy, breaker, & caps     │
│  - Builds verified calldata & broadcasts│
└─────────────────────────────────────────┘
```

## Sentient UID isolation & residual risk

For sentient profiles, all private keys (`wallet.json`), policy configuration (`sentient-policy.toml`), spend ledger records (`sentient-spend.json`), policy audit entries (`policy-audit.jsonl`), and the kill-switch marker (`signerd-tripped.json`) reside in `/var/lib/vaughan/sentient` owned exclusively by the `vaughan-signer` system user (mode `0700`/`0600`); one-time `policy-approval.json` files (when present) are owned by `root:vaughan-signer` (or root and the store's group if `vaughan-signer` does not exist), mode `0440`. The agent process runs under the normal host UID and communicates with `vaughan-signerd` strictly via `/run/vaughan/signerd.sock`.

The daemon validates peer credentials on every connection via `SO_PEERCRED` against `allow_uid`. The agent process cannot read or alter private keys, policy limits, or spend ledgers, and cannot submit arbitrary calldata (untyped tools return `intent_untyped` and never reach the signer socket).

Live policy updates (`vaughan --profile sentient policy set|confirm-unsafe` or the TUI policy screen) use out-of-band root approval inside `vaughan-signerd`: `{"op":"policy_update","auth":"stage"}` writes `policy-pending.json` (0600, 10-minute TTL); a human operator running `/usr/local/bin/vaughan signerd approve` as root (from a clean root console, `run0`, or `sudo` from a separate admin account) inspects the on-disk policy and `policy-pending.json` on an interactive TTY (`stdin.is_terminal()`) and writes one-time `policy-approval.json` (mode `0440`, `root:vaughan-signer` or the store's group if `vaughan-signer` does not exist); `{"op":"policy_update","auth":"approved"}` opens `policy-approval.json` with `O_RDONLY | O_NOFOLLOW`, verifies `fstat` (`uid == 0`, `nlink == 1`, `mode == 0440`), unlinks it before apply, rejects stale `expected_digest`, and appends field diffs to `policy-audit.jsonl`. The legacy socket password step-up path was removed in S3c (`auth: None` returns `step_up_unsupported` without reading or unlocking `wallet.json`, and unknown `"password"` keys fail JSON deserialization).

**What root approval defends against**: a compromised process running as the `vaughan` agent UID (or any other non-root, non-`vaughan-signer` UID on the host) cannot widen `sentient-policy.toml` on its own — it can only stage `policy-pending.json` and wait for a `root`-owned (`uid == 0`, `0440`, `O_NOFOLLOW`, `nlink == 1`) `policy-approval.json` bound to `(old_digest, new_digest)` with a short TTL.

**What root approval does NOT defend against**: (a) a process that already has root execution (passwordless `sudo` on the compromised UID, a root-owned binary it can write, or an already-compromised root session); (b) running `sudo vaughan signerd approve` from a shell owned by a compromised agent UID, because that UID's `~/.bashrc`, `PATH`, or shell functions can intercept the command before `sudo` runs (`approve` checks `euid == 0` and `stdin.is_terminal()`, not that the controlling terminal is clean) — run `/usr/local/bin/vaughan signerd approve` from a root login or a separate admin account; (c) `ptrace`/keystroke injection against the approving terminal; (d) code execution as `vaughan-signer`, which owns the store and `sentient-policy.toml` and can rewrite it directly without going through the socket.

Adviser profiles retain the loopback TCP / `mcp.session` token path where explicit human approval on screen is the security control.
