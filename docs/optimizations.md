# Optimization Provenance

> Policy: every optimizer piece of code added to Vaughan records where it
> comes from. **EIP specs are the preferred source**; battle-tested libraries
> and canonical contracts come second; other wallets (Rabby, MetaMask) are
> pattern references to revisit later, not code sources.

## Auto asset detection

| Piece | Where it comes from | Notes |
|---|---|---|
| `balanceOf` / `decimals` / `symbol` / `name` | **EIP-20** — https://eips.ethereum.org/EIPS/eip-20 (metadata accessors per OpenZeppelin's ERC20Metadata) | All four are best-effort: EIP-20 only requires `balanceOf` + `transfer` etc.; a token without `symbol()` falls back to the registry / shortened address |
| Batch balance reads | **Multicall3** (mds1) — https://github.com/mds1/multicall, `tryAggregate` with `requireSuccess=false` | One `eth_call` for all curated tokens; per-call success means one weird token can't fail the batch. Contract `0xcA11bde05977b3631167028862bE2a173976CA11` — verified deployed on **both** PulseChain mainnet and testnet (`cast codesize` → 3808 on each, 2026-08-18). Returns `Result[]` (`(bool success, bytes returnData)` structs per call, matching the canonical ABI). The adapter probes `get_code_at` before batching — a chain *without* Multicall3 falls back to sequential `balanceOf` reads (an `eth_call` to an empty address returns `0x` as success, so the probe is what distinguishes the two paths) |
| Curated token list | Verified on-chain 2026-08-18 (`cast` `symbol()`/`decimals()` against rpc.pulsechain.com), cross-checked against api.scan.pulsechain.com | Addresses are the trusted seed; symbol/decimals are re-read from the contract at scan time (cached 1 h). WPLS matches the DEX project's `docs/addresses.md` |
| Metadata + balance caches | moka (battle-tested Rust cache) | 1 h TTL for metadata, 10 s for balances (same policy as the native balance cache) |
| Transfer-event token discovery | Pattern: Rabby / MetaMask scan ERC-20 `Transfer` logs for the address | **Not implemented yet** — the curated list is the Phase-1 scope; log scanning is the follow-up (same getLogs approach as the browser-engine `Probe`) |
| TUI assets view | Vaughan's own view layer (ratatui) over `WalletState::assets` | Dashboard shortcut `a` (or Tab-cycle) → lists every detected balance; `r` refreshes, `Esc`/`d` returns |

## Auto gas calculation

| Piece | Where it comes from | Notes |
|---|---|---|
| `estimate_gas` | **alloy** `Provider::estimate_gas` (ethers-rs lineage) | Battle-tested EVM gas estimation |
| EIP-1559 fee estimate | **alloy** `Provider::estimate_eip1559_fees` (feeHistory-percentile algorithm, EIP-1559 — https://eips.ethereum.org/EIPS/eip-1559) | The same algorithm ethers-rs/MetaMask use. Falls back to the base-fee × 2 + tip heuristic (audit 4.2: PulseChain's sub-gwei market → 0.01 gwei default tip), then to legacy gas price, when feeHistory is unavailable |
| Fee display before sign | TUI send confirm (`Fee: …` + `max X gwei/gas · limit N`) + provider `describe_tx` | Shown for every send; native `send` path estimates and displays the total **and** the per-gas breakdown before broadcast |
| Fallback RPCs | alloy HTTP provider pool (adapter `with_provider`) | Transport failures retry across endpoints; gas estimation failures count as transport errors |

## Verification

- Unit: registry tests (addresses, per-chain selection), cache/fallback logic.
- Integration: `vaughan-core/tests/assets_e2e.rs` — anvil fork of **PulseChain mainnet**,
  wrap PLS→WPLS, then assert `get_assets` returns native PLS + WPLS, both via the
  **Multicall3 batch** and via the **sequential fallback** (Multicall3's code
  wiped with `anvil_setCode` — same result set on both paths).
- TUI: `vaughan-tui/tests/assets_view.rs` — headless `TestBackend` render of the
  assets view (balances, refresh, navigation, error state).
