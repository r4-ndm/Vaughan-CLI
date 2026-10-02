# Ideas borrowed from ZKX Wallet — implementation plan

> **Status: PLANNED** (2026-10-02). Not started.
> Source: inspection of the public ZKX Web3 Wallet extension v0.0.50
> (`fibfaangghabdbndamiaceoahiajfgck`), which is a fork of the Ambire extension
> plus the RAILGUN TypeScript SDK. Unpacked copy lives outside the repo at
> `~/Desktop/zkx-wallet-src/`.
>
> **Code-source rule (CLAUDE.md engineering rule 5):** ZKX is proprietary and
> Ambire's `ambire-common` is GPL. Both are *idea and interface references only*.
> Nothing is copied or translated; every line is written fresh from EIPs,
> verified contract ABIs, and public API docs.

## Summary

| # | Idea | New deps | Moves funds | Effort | Order |
|---|------|----------|-------------|--------|-------|
| 1 | Transaction humanizer (calldata + revert errors → plain English) | None | No | Medium | First |
| 2 | Balance-change preview before signing | None | No | Small–Med (gated on RPC spike) | Second |
| 6 | Phishing / malicious-address deny-list | None | No | Small | Second (with 2) |
| 3 | Gasless swaps via signed permits + relayer | None | Yes | Small–Med | Third (blocked on API docs) |
| 4 | RAILGUN shielded balances (own derivation tree) | Yes — approval needed | Yes | **Large (multi-month)** | Spike + go/no-go |
| 5 | Private gas payment for shielded and stealth sends | Depends on 4 | Yes | Med | With 4; stealth part can go early |

Ideas 1, 2 and 6 strengthen the existing approval gate (rule 5: every signing
path shows the full request). Idea 3 adds a third-party API. Idea 4 reverses a
recorded NO-GO and needs dependency approval before any code.

> **Reviewed 2026-10-02 against the codebase.** Corrections from that review:
> - Idea 2 was over-scoped: Vaughan already re-runs `eth_call` at approve time
>   (FR-6.4), so only the token-diff display is new. Dropped the
>   `debug_traceCall` prestate tier; `eth_simulateV1` + quote fallback is enough.
> - Idea 3 was over-scoped: EIP-712 signing already works across software,
>   Ledger and Trezor (`sign_typed_data`, `EvmTypedData`). Gasless is mostly a
>   relayer client + config + a new proposal type.
> - Idea 4 (RAILGUN) was under-scoped: ZKX ships 51 MB of circuit artifacts and
>   a ~6.4 MB prover, and the Rust-only rule means reimplementing the proving
>   stack. It is a multi-month effort, not a phase.
> - Idea 6 (phishing deny-list) was missing entirely: ZKX has domain/address
>   blacklists, Vaughan has none, and the MCP model makes it important.

---

## 1. Transaction humanizer

**Goal:** every approval card (MCP proposal, VB / provider `eth_sendTransaction`,
TUI send/swap confirm) shows one plain sentence like *"Swap 100 USDC for at least
0.03 WPLS on PulseX V2, recipient = you"*, plus warnings, derived only from
calldata — never from agent text.

**Today:** `core/proposal_review.rs` decodes ERC-20 `transfer`/`approve`, WPLS
`deposit`/`withdraw` and HEX staking for MCP proposals only.

### Design

- New module `vaughan-core/src/core/humanizer/`:
  - `mod.rs` — `humanize(chain_id, from, to, value, calldata) -> Humanized`
    returning `{ summary: String, rows: Vec<VerifyRow>, warnings: Vec<Warning> }`.
  - `modules/` — one small decoder per protocol, each a pure function behind a
    `HumanizerModule` trait (single-purpose rule):
    `erc20`, `erc721`, `weth`, `permit` (EIP-2612 + Permit2), `uniswap_v2_router`
    (PulseX V1/V2, 9mm, wiz4rd), `uniswap_v3_router`, `piteas`, `hex_stake`,
    `ambire_batch` (decode each call inside a 7702 batch recursively).
  - `labels.rs` — address → name from existing catalogs (`dex_catalog`,
    `token_origin`, `dex_routers`, custom tokens). No hosted lookup.
  - `errors.rs` — revert decoding: `Error(string)`, `Panic(uint256)` codes, and
    known custom errors from the same ABIs; plus common RPC failures
    (insufficient funds, nonce too low, underpriced) mapped to short messages.
- Warnings (each a typed enum, not free text): unlimited approval, approval to
  an unknown spender, recipient ≠ sender on a swap, value sent to a contract
  with no matching function, `setApprovalForAll(true)`, permit with
  far-future deadline.
- `proposal_review.rs` becomes a thin caller of the humanizer; its existing
  tests must keep passing unchanged.
- Unknown calldata falls back to today's behaviour: selector + raw args via
  the `wiz4rd-engine` signature lookup, clearly labelled "not recognised".

### Surfaces

- MCP proposal card (Advisor) — replace hand-built rows.
- Provider approval (`vaughan-provider` → TUI `ApprovalKind`) for dApp txs.
- TUI Send / Ag / Dex / Wrap confirm screens and the Approvals view.
- Revert messages in flashes and `WalletError::user_message`.
- MCP read tool `humanize_tx` so agents can show the same text they will be
  judged by (output still labelled as Vaughan-derived).

### Tests

- Table-driven unit tests per module with real mainnet calldata fixtures
  (hex only, no secrets).
- Anvil test: build each supported tx type, humanize, assert summary.
- Snapshot tests of approval cards via `TestBackend`.

---

## 2. Balance-change preview

**Goal:** the confirm screen shows *"You send −100 USDC, you receive ≈ +0.031
WPLS, gas ≈ 0.4 PLS"* computed by simulation, not by trusting the quote.

**Today:** assets already use Multicall3 batching, so Ambire's "deployless"
balance reader adds little. What is missing is a **before/after diff**.

### Design

- `vaughan-core/src/core/simulate_diff.rs`:
  1. Collect tokens of interest: tokens named by the humanizer output, plus the
     native coin, plus the user's tracked assets.
  2. Preferred path: `eth_simulateV1` — it returns per-call balance changes
     directly, which is exactly the diff we need. Probe once per RPC, cache in
     `moka`.
  3. Fallback: show the humanizer's quoted amounts labelled "quote, not
     simulated". (Revert-detection is already covered by the existing
     approve-time `eth_call` re-simulation, FR-6.4, so the diff is the only
     new part.)
- A `debug_traceCall` prestate-tracer tier is deliberately **out of scope**:
  few public PulseChain RPCs expose it and `eth_simulateV1` covers the need.
- Rows feed into the same `VerifyRow` table as idea 1; a mismatch between
  quote and simulation beyond slippage becomes a red warning.
- Re-run at approve time alongside the existing re-simulation and fee-spike
  guard; a changed diff forces re-confirmation.

### Gate (decides the size of this item)

- **Spike first:** which PulseChain / ETH RPCs support `eth_simulateV1`. If
  the main PulseChain RPCs do, this is a small feature. If not, the diff is
  quote-only on PulseChain and the item shrinks further. Record results here
  before coding.

---

## 3. Gasless swaps (signed permit + relayer)

**Goal:** a user holding tokens but no PLS can swap by signing EIP-712 messages;
a relayer submits and pays gas.

**Pattern seen in ZKX:** permit types `Permit` (EIP-2612), DAI-style permit,
`TransferWithAuthorization` (EIP-3009) and a `SwapParams` struct; submit to a
relayer, poll status by correlation id.

### Phase 0 — prerequisites (human, before code)

- [ ] Get **official API docs** for the LibertySwap gasless relayer (or another
      PulseChain relayer). Do not build against endpoints reverse-read from the
      extension.
- [ ] Confirm testnet 943 support and the verifying contract address.
- [ ] Confirm what data the relayer receives and logs (no-telemetry rule:
      feature must be opt-in and disclosed).

### Design (after Phase 0)

EIP-712 typed-data signing already exists across software, Ledger and Trezor
(`security::signing::sign_typed_data`, `SignRequest::EvmTypedData`), so the
signing side is done. What is new is the relayer client and the proposal type.

- `vaughan-core/src/core/gasless/`: `client.rs` (reqwest), `types.rs`,
  `typed_data.rs` (build EIP-712 via `alloy-dyn-abi`), `config.rs`
  (opt-in flag, relayer URL, per-network enable).
- Flow: quote → build typed data → humanizer renders every field (idea 1) →
  explicit approval → sign (reuse existing software / Ledger / Trezor typed-data
  paths) → submit → poll → show final tx hash.
- Guards: reject unlimited permits, reject deadline > 1 hour, verify
  `verifyingContract` against config, verify signed amounts equal the quote.
- Surfaces: Ag view toggle "gasless" (only shown when PLS balance is low and
  the token supports a permit), CLI `vaughan swap --gasless`, MCP
  `propose_gasless_swap` (propose-only, same approval queue).

### Tests

- Mock HTTP relayer; EIP-712 digest vectors per permit type; Anvil test that
  the signed permit is accepted by a deployed EIP-2612 token.

---

## 4. RAILGUN shielded balances

**Why revisit:** `docs/kohaku-go-no-go.md` blocks on *"BIP-32 vs babyjubjub
seed tree"*. ZKX shows the fix is not to reconcile the trees: feed the same
BIP-39 mnemonic to RAILGUN's own derivation and keep it separate.

- Spending: `m/44'/1984'/0'/0'/{i}'`, viewing: `m/420'/1984'/0'/0'/{i}'`,
  master key from `HMAC-SHA512("babyjubjub seed", bip39_seed)`, hardened-only
  children (SLIP-10 style, not secp256k1 BIP-32).
- With a BIP-39 passphrase, ZKX derives a second mnemonic from
  `sha256(seed)` as entropy. Vaughan should instead decide one rule and freeze
  it in a spec doc (`docs/railgun-keys.md`), with test vectors checked against
  the official RAILGUN engine so wallets restore in Railway / other clients.

### Hard constraints

- **Rust only, no Node.** The RAILGUN TS SDK cannot be used. This is the
  dominant cost: ZKX leans on the full RAILGUN TypeScript SDK (proving, merkle
  sync, note scanning, POI), and there is no mature Rust equivalent. Vaughan
  would have to build or adopt Rust crates for all of it.
- **Size:** ZKX ships ~51 MB of circuit artifacts (113 files) and a ~6.4 MB
  prover bundle. Expect a comparable artifact footprint plus the prover.
- **Realistic effort: multi-month**, not a phase. Treat the whole item as a
  separate project gated behind the spike, not as "phase 1+ if GO".
- **Network:** RAILGUN is deployed on Ethereum, Arbitrum, BNB Chain, Polygon
  (+ Sepolia / Amoy testnets). **Not PulseChain.** Feature is non-Pulse only.
- **New crypto deps** are required (babyjubjub, Poseidon, Groth16 prover,
  circuit artifact loading). None are on the allowlist → explicit approval.

### Phase 0 — spike + go/no-go (no merged code)

- [ ] Inventory Rust crates for babyjubjub / Poseidon / Groth16
      (e.g. arkworks family); check audits, licences, maintenance.
- [ ] Re-read `~/Desktop/Kohaku-rs` as a *reference only* for structure.
- [ ] Prototype key derivation in a scratch crate; match official RAILGUN
      engine vectors for 0zk address and viewing key.
- [ ] Measure proof time and artifact size (ZKX ships ~60 MB of circuits).
- [ ] Write `docs/railgun-go-no-go.md` with a verdict; update
      `kohaku-go-no-go.md` and FR-3.4.

### Phase 1+ (only if GO)

- `vaughan-core/src/security/railgun_keys.rs` — derivation, secrets in
  `secrecy` + `zeroize`, keys never persisted unencrypted.
- `vaughan-core/src/core/railgun/` — merkle tree sync + note scanning
  (background job), shield, private transfer, unshield.
- Artifact download on first use with pinned SHA-256 hashes; stored in
  `dirs` data dir.
- Proof-of-Innocence (POI) node support, user-configurable list.
- TUI: Dashboard public/private toggle; Send `0zk…` recipients; shield /
  unshield actions. CLI and MCP read tools first, write tools propose-only.
- Sepolia first (testnet-first rule), then Ethereum / Arbitrum.

---

## 6. Phishing / malicious-address deny-list (added on review)

**Gap found on review:** ZKX ships domain and contract blacklists / scam
detection. Vaughan has **none** — the only mention is a comment in
`vaughan-provider` noting that `chainId` + `verifyingContract` "are what a
phishing signature abuses". Because ideas 3 (gasless) and the whole MCP model
involve signing things an *agent* or *dApp* proposed, a local deny-list is
high-value and cheap.

### Design

- `vaughan-core/src/core/denylist.rs`:
  - A versioned, bundled list of known drainer / scam contract addresses and
    EIP-712 `verifyingContract` values (sourced from public, permissively
    licensed lists; curated, no hosted fetch — no-telemetry rule).
  - Optional user-added entries in the profile dir.
  - Checked by the humanizer (idea 1) and at provider-approval time: any tx or
    typed-data whose `to` / `spender` / `verifyingContract` is listed gets a
    hard red warning and requires an extra explicit confirm.
- No network calls, no address uploads — the list ships with the binary.

### Tests

- Unit: listed address triggers warning; unlisted does not; user entries merge.
- Provider approval test: listed `verifyingContract` forces the extra confirm.

---

## 5. Private gas payment

**The ZKX weakness to avoid:** private sends use `sendWithPublicWallet = true`
and `showSenderAddressToRecipient = true`, so the user's public address pays
gas and appears on-chain next to the shielded tx.

### Design

- **RAILGUN (with idea 4):** default to RAILGUN broadcasters (fee paid from the
  shielded balance). Public-wallet self-relay only behind an explicit
  "this links your public address" confirmation. `showSenderAddressToRecipient`
  defaults to off.
- **ERC-5564 stealth (can ship early, no new deps):** today a stealth sweep
  needs gas on the stealth address; funding it from the main wallet links the
  two. Options to evaluate:
  - Sweep via EIP-7702 batch from the stealth key with a relayer paying gas
    (reuses `vaughan-aa`; pairs with idea 3's relayer if it supports this).
  - Sweep ERC-20s with a permit to a gasless relayer (idea 3).
  - At minimum: warn on the sweep screen when the gas funder is a known
    wallet of the user.
- Humanizer warning (idea 1): "this transaction links address A to address B".

---

## Suggested sequencing

1. **Humanizer** (1) — no deps, improves every existing approval path.
2. **Balance diff** (2) — gated on the `eth_simulateV1` RPC spike; build on
   humanizer rows.
3. **Phishing deny-list** (6) — small, feeds the humanizer and provider approval.
4. **Stealth-sweep linkage warning** (5, stealth part) — small, immediate.
5. **Gasless** (3) — as soon as relayer API docs arrive; EIP-712 signing is
   already done, so this is mostly a relayer client.
6. **RAILGUN spike** (4) — in parallel; a genuine go/no-go, not a formality.
7. **RAILGUN build + broadcaster gas** (4 + 5) — only on GO, and planned as a
   multi-month effort.

## Requirements to add on start

- FR-9.1 humanizer, FR-9.2 balance diff, FR-9.3 gasless, FR-9.4 phishing
  deny-list, and updates to FR-3.4 (RAILGUN) in `REQUIREMENTS.md` when each
  item starts.
