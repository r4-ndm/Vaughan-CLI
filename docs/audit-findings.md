# Vaughan-CLI — Audit Findings & Technical Review

**Date**: August 2026  
**Target Architecture**: Rust Multi-Chain CLI Wallet TUI (Alloy 1.7, Ratatui, Kohaku-rs, Ambire AA, Freedom Browser)

---

## 1. Executive Summary

Vaughan-CLI demonstrates a clean, layered architecture with strong security fundamentals:
- Clear separation between `vaughan-core` (domain logic/crypto/chains), `vaughan-provider` (loopback EIP-1193 JSON-RPC server), and `vaughan-tui` (Ratatui frontend).
- Security hygiene is respected: `Argon2id` + `AES-256-GCM` for vault encryption, `secrecy::SecretString` for password parameters, and `zeroize` on mnemonic/key drop.
- Pluggable multi-chain abstraction via `ChainAdapter` trait with tagged payloads.
- Custom `vaughan_signTransaction` alongside standard EIP-1193 methods specifically designed for the Freedom Browser signer backend.

This document aggregates specific architectural, security, performance, ergonomics, and protocol findings to be addressed.

---

## 2. Critical & High Priority Issues

### 2.1 Main UI Thread Blocking on Async RPC Calls (`handle.block_on`)
- **Locations**:
  - `vaughan-tui/src/app.rs` (`navigate`)
  - `vaughan-tui/src/views/send.rs` (`estimate`, `send`)
  - `vaughan-tui/src/views/dashboard.rs` (`refresh`)
- **Problem**:
  Async network operations (such as `wallet.balance()`, `wallet.estimate_fee()`, `wallet.send()`) are executed synchronously on the main thread via `handle.block_on(...)`.
- **Impact**:
  In a TUI event loop, `block_on` halts crossterm event polling and frame rendering. If an RPC endpoint (PulseChain public RPC or Sepolia) experiences latency or goes offline, the entire CLI freezes (unresponsive keyboard input, no UI updates).
- **Recommendation**:
  Spawn async tasks via `tokio::spawn` and communicate results back to the view using an async channel (`tokio::sync::mpsc`) or a shared state struct with a loading spinner flag.

---

### 2.2 Global `'q'` Key Interception Kills App During Text Input
- **Location**: `vaughan-tui/src/app.rs` (lines 148–157)
- **Code Snippet**:
  ```rust
  let quit = match key.code {
      KeyCode::Char('q') => true,
      KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
      _ => false,
  };
  if quit {
      self.quitting = true;
      return;
  }
  ```
- **Problem**:
  `'q'` is checked at the root app level before delegating key events to the active view.
- **Impact**:
  - Entering a password containing `'q'` immediately exits the application.
  - Entering or restoring a 12-word mnemonic phrase containing words like `quick`, `quiet`, `quote`, `quantum`, `quality` exits the application immediately.
  - Entering a recipient address or hex string containing `'q'` exits the application.
- **Recommendation**:
  Remove global un-modified `'q'` interception when an input field is active, or standardize quitting on `Ctrl+C` and `Ctrl+Q` globally.

---

### 2.3 Global `Tab` Key Intercepts Form Field Navigation
- **Location**: `vaughan-tui/src/app.rs` (lines 160–172)
- **Problem**:
  `KeyCode::Tab` is intercepted globally to cycle screens (`Dashboard -> Send -> Receive -> Settings`).
- **Impact**:
  On forms like `SendView` (which contains Recipient and Amount inputs), pressing `Tab` to switch focus between inputs switches the active screen to `Receive` instead.
- **Recommendation**:
  Pass `Tab` to the active view's `handle_key` first; only execute screen switching if the active view does not consume the `Tab` event.

---

### 2.4 Nonce Caching (5s TTL) in `EvmAdapter`
- **Location**: `vaughan-core/src/chains/evm/adapter.rs` (`nonce_cache`, lines 42, 125–137)
- **Problem**:
  Transaction nonces are cached with a 5-second TTL in `moka::future::Cache`.
- **Impact**:
  If a user or connected dApp submits multiple transactions in rapid succession (< 5s), the cached nonce is reused, resulting in RPC errors (`nonce too low`) or transaction collisions.
- **Recommendation**:
  For transaction submission, bypass `nonce_cache` and query the pending nonce directly with `BlockNumberOrTag::Pending`, or track the local pending nonce in memory.

---

## 3. Security & Key Hygiene Enhancements

### 3.1 Strict POSIX Permissions on Vault File (`0o600` / `0o700`)
- **Location**: `vaughan-core/src/core/persistence.rs` (`StateManager::save`)
- **Problem**:
  `fs::write` creates `<data_dir>/vaughan-cli/wallet.json` with standard umask permissions (e.g. `0o644`), making the file readable by all users on a multi-user Linux/Unix machine.
- **Recommendation**:
  Explicitly set POSIX permissions on Unix:
  - Directory: `0o700` (`rwx------`)
  - Vault file (`wallet.json` & `.tmp`): `0o600` (`rw-------`)
  ```rust
  #[cfg(unix)]
  {
      use std::os::unix::fs::PermissionsExt;
      fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
  }
  ```

---

### 3.2 EIP-712 Cross-Chain Domain ChainId Validation
- **Location**: `vaughan-provider/src/methods.rs` (`parse_sign_typed_data`)
- **Observation**:
  `eth_signTypedData_v4` receives structured EIP-712 data with a `domain` containing a `chainId`.
- **Recommendation**:
  Before prompting the user for approval, verify that `domain.chainId` matches the active wallet chain ID (or surface a clear cross-chain warning) to prevent cross-chain replay attacks.

---

## 4. Performance & Optimization Opportunities

### 4.1 Repeated PBKDF2 Computations on Unlock
- **Locations**:
  - `vaughan-core/src/core/account.rs` (lines 50–56)
  - `vaughan-core/src/security/hd_wallet.rs` (lines 38–48)
- **Problem**:
  ```rust
  pub fn derive_account(mnemonic: &Mnemonic, index: u32) -> Result<PrivateKeySigner, WalletError> {
      let seed = mnemonic.to_seed(""); // PBKDF2 (2048 iterations) executed on every call
      let path = format!("{ETH_DERIVATION_PATH}/{index}");
      ...
  }
  ```
  `AccountManager::with_active` derives `count` (default 10) accounts in a loop, re-running PBKDF2 10 times consecutively.
- **Recommendation**:
  Derive the 64-byte seed once, create the root `XPrv::new(seed)` master key, derive the parent path `m/44'/60'/0'/0`, and derive child accounts from the parent key. This provides a ~10x speedup on wallet unlock.

---

### 4.2 PulseChain Priority Fee Tuning & RPC Fallbacks
- **Location**: `vaughan-core/src/chains/evm/adapter.rs` (line 251) and `networks.rs`
- **Observations**:
  1. EIP-1559 estimation hardcodes `priority_fee = 1_500_000_000` (1.5 gwei). On PulseChain (369), gas prices are often fractions of a gwei (Beats), so 1.5 gwei can result in overpaying.
  2. Public RPCs (`https://rpc.pulsechain.com`) can experience temporary rate limits.
- **Recommendations**:
  - Support network-specific default priority fees in `EvmNetworkConfig`.
  - Add fallback RPC endpoint URLs to `EvmNetworkConfig` (e.g. `https://pulsechain-rpc.publicnode.com`).

---

## 5. Web3 Compatibility & Protocol Robustness

### 5.1 Inverted Parameter Order Handling for `personal_sign`
- **Location**: `vaughan-provider/src/methods.rs` (lines 272–301, `parse_personal_sign`)
- **Problem**:
  In the wild, different dApp libraries pass parameters to `personal_sign` inconsistently:
  - Standard EIP-1193: `[message, address]`
  - Legacy web3.js / older dApps: `[address, message]`
- **Recommendation**:
  Add an auto-detection swap:
  ```rust
  let (message, address) = if is_address_like(&array[0]) {
      (array[1].to_string(), array[0].to_string())
  } else {
      (array[0].to_string(), array[1].to_string())
  };
  ```
  This eliminates mysterious signing rejections on legacy dApps.

---

### 5.2 Ambiguous Gas Parameters in EVM Adapter
- **Location**: `vaughan-core/src/chains/evm/adapter.rs` (lines 322–337)
- **Problem**:
  If a dApp sends both `gasPrice` and `maxFeePerGas` in a transaction payload, both are populated on `TransactionRequest`.
- **Recommendation**:
  Enforce precedence: only populate `req.gas_price` if `max_fee_per_gas` is `None`.

---

### 5.3 Terminal Panic Hook (Raw Mode Cleanup)
- **Location**: `vaughan-tui/src/main.rs`
- **Problem**:
  If the application panics while crossterm is in raw mode and alternate screen, the terminal shell is left in a broken/unusable state (invisible cursor, raw echoing).
- **Recommendation**:
  Install a custom panic hook or use `ratatui::init()` / `ratatui::restore()` to ensure the terminal restores normal mode cleanly even on unexpected panic.

---

### 5.4 Network Switching Event Dispatch to dApps
- **Location**: `vaughan-tui/src/views/settings.rs` (lines 80–90)
- **Observation**:
  When switching active networks in Settings, `wallet.set_active_network(&id)` is updated, but `ProviderEvent::ChainChanged` is not published to `EventBus`.
- **Recommendation**:
  Emit `EventBus::publish(ProviderEvent::ChainChanged(format!("0x{:x}", chain_id)))` on network changes so connected dApps / Freedom Browser update their chain ID dynamically without a browser refresh.

---

### 5.5 Ergonomics: Input Widget Navigation & Editing Keys
- **Location**: `vaughan-tui/src/input.rs` (lines 33–46)
- **Problem**:
  `Input::handle_key` only handles `Enter`, `Char`, and `Backspace`.
- **Missing**:
  - `KeyCode::Left` / `KeyCode::Right` for cursor repositioning
  - `KeyCode::Delete` for forward deletion
  - `KeyCode::Home` / `KeyCode::End`
  - Clipboard paste support
- **Recommendation**:
  Add an internal cursor index to `Input` to allow in-place editing when entering 12-word mnemonics or long recipient addresses.

---

## 6. Action Item Checklist

- [x] **Fix Global `'q'`**: Restrict single-character `'q'` quit handling to non-input views; use `Ctrl+C`/`Ctrl+Q` globally. Views report `KeyOutcome::Consumed` for keys they use; bare `'q'` quits only when the active view did not consume it. (Fixed — `vaughan-tui/src/app.rs` + all views.)
- [x] **Fix `Tab` Key Flow**: Delegate `Tab` to view inputs before cycling screens. `SendView` now uses Tab to move between Recipient/Amount; the app-level cycle fires only when the view does not consume Tab. (Fixed — `vaughan-tui/src/app.rs`, `views/send.rs`.)
- [ ] **Non-blocking TUI**: Replace `handle.block_on` in TUI views with async task spawning and channel messages. *(Partially done — the FR-2.3 approval flow now bridges the provider server to the UI over an MPSC + `oneshot` channel, so dApp sign/send requests no longer block the handler. The remaining `handle.block_on` calls in the dashboard/send views (`balance`, `estimate_fee`, `send`) still run inline on the UI thread.)*
- [x] **Set POSIX `0o600` Permissions**: Restrict `wallet.json` and directory permissions on Unix. Directory `0o700`, vault + temp file `0o600`, with a permissions unit test. `load()` also locks down pre-existing vaults written before this rule (best effort, non-fatal). (Fixed — `vaughan-core/src/core/persistence.rs`.)
- [x] **Remove Nonce Caching**: Query `BlockNumberOrTag::Pending` directly during transaction creation. `send_transaction` now queries the pending nonce directly, bypassing the 5s-TTL cache. (Fixed — `vaughan-core/src/chains/evm/adapter.rs`.)
- [x] **Optimize HD Derivation**: Cache master/parent derivation to eliminate redundant PBKDF2 iterations. `derive_account_parent` derives `m/44'/60'/0'/0` once; account unlock derives children from it (~10x fewer PBKDF2 runs). (Fixed — `vaughan-core/src/security/hd_wallet.rs`, `core/account.rs`.)
- [x] **Support `personal_sign` Param Order**: Auto-detect `[data, address]` vs `[address, data]` by address shape in slot 0. (Fixed — `vaughan-provider/src/methods.rs`, with a legacy-order test.)
- [x] **Enhance `Input` Widget**: Add cursor navigation (`Left`/`Right`/`Delete`/`Home`/`End`) with a cursor-aware render and `InputAction` results. (Fixed — `vaughan-tui/src/input.rs`.)
- [x] **Terminal Panic Hook**: Ensure clean terminal restore on unexpected panics. *No change needed — `ratatui::init()` already installs a panic hook that restores the terminal (verified in ratatui 0.29 `terminal/init.rs`).*
- [x] **Wire `EventBus` to Settings**: Emit `ChainChanged` on network change. Settings publishes `ProviderEvent::ChainChanged` via the app-owned `EventBus`; lock/unlock also publish `accountsChanged` (`[]` on lock, active address on unlock). (Fixed — `vaughan-tui/src/views/settings.rs`, `app.rs`, `views/dashboard.rs`, `views/unlock.rs`.)

### Still open (not part of the checklist)

- **2.1 Non-blocking TUI** — the provider→UI bridge is now channel-based (done with FR-2.3); the remaining piece is replacing `handle.block_on` in the dashboard/send views (balance/estimate/broadcast) with `tokio::spawn` + channel messages.

### Recently closed (was in §4, not the checklist)

- [x] **4.2 PulseChain priority-fee tuning + RPC fallbacks** — `EvmNetworkConfig` now carries `default_priority_fee_wei` and `fallback_rpc_urls` (serde-defaulted). PulseChain mainnet/testnet use 0.01 gwei tips (vs Ethereum's 1.5 gwei); Polygon/BSC/Base get chain-appropriate defaults. The EVM adapter builds a primary + fallback provider chain with a sticky last-known-good endpoint, retrying only transport-ish failures (RPC/network/gas/broadcast) and failing fast on local errors; broadcasting to a fallback is safe (same nonce → on-chain no-op). Covered by unit tests (classification + fallback loop, offline). (Fixed — `vaughan-core/src/chains/evm/networks.rs`, `chains/evm/adapter.rs`, `core/wallet.rs`, `chains/mod.rs`.)
- [x] **Gas Precedence**: Only set `gas_price` if `max_fee_per_gas` is absent. (Fixed — `vaughan-core/src/chains/evm/adapter.rs`.)

---

## 7. 2026-11-20 session — codebase organization pass

Scope was agent-navigability and governance tooling (new `.claude/skills/audit/`,
`docs/` reorg, module docs, size-budget cleanup), not a full forensic re-audit. Per
`docs/audit-playbook.md`'s evidence rules: this is what was verified vs. what wasn't.

**Mechanical gate**: `scripts/audit.sh --with-tests --binary` at commit `50fd36d5` plus
this session's changes — fmt/clippy (both feature sets)/tests/cargo-audit/tracked-file
hygiene/secrets scan all green; `scripts/size_budget.py` at 0 violations after reverting
a module-doc addition that would have grown an already-frozen file (see finding below).

**Spot-checked invariants** (read-only; not a full Pass A diff read):
- No `siphon` module exists (`git grep -ri siphon` matches only the playbook itself) — P0 invariant 1/8 holds.
- `vaughan-hl/Cargo.toml` has no `vaughan-core` dependency — custody boundary (invariant 11) holds.
- `vaughan-agent::sentient::trader::ensure_local_signing_allowed` still rejects the
  `sentient`/`degen` profile names for local signing — unchanged from the documented design.
- Default `cargo build -p vaughan-cli` still excludes `railgun-wallet`/`broadcaster-core`/`wasmer` (`cargo tree`) and the installed binary has 0 hits for `private (RAILGUN)`.

**P2 finding — module-doc requirement conflicts with the size-budget ratchet.**
`vaughan-tui/src/views/lp/{confirm,helpers,input,render,state}.rs` are missing the `//!`
module doc `AGENTS.md` rule 4 requires, but all five are already at their frozen ceiling
in `scripts/size-budget.txt`; adding even a 3-line doc comment trips `scripts/audit.sh`'s
"GREW file" check. Not fixed this session — tracked in `TASKS.md` under "Codebase
organization" to land once those files are split (smaller resulting files can take the
doc without raising the ratchet).

**Not covered**: a full Pass A line-by-line read of money paths across `vaughan-core`
(41k lines) — this session's diff was docs/tooling/comments only, no logic changes, so
Pass A was limited to the spot-checks above rather than a full re-derivation.

---

## 8. 2026-11-20 second pass — `spawn_job` / `tui.rs` split verification

Pass A on the two splits from §7, run as a mechanical proof rather than a read-through.
The scripts are `scripts/split-proof-hl-tui.py` and `scripts/split-proof-app-jobs.py`
(both take a base rev, default `HEAD`), following the precedent of the existing
`private/split-proof.sh`. **They are local-only** — `/private/` is gitignored and
`scripts/audit.sh` fails if anything there becomes tracked — so these results are
reproducible on this machine but are *not* in the repo or CI. Promote them to `scripts/`
if the proof should gate future splits; noted in `TASKS.md`.

Each proof was negative-tested rather than trusted: transposing two same-typed `String`
arguments in a send path (`build_erc20_transfer(&token, &amount, &to)`) makes
`split-proof-app-jobs.py` report a real body difference and exit non-zero, so a green run
is not vacuous.

**Move-only is proven, not asserted:**

- `vaughan-hl/src/tui.rs` → `tui/{mod,render}.rs`: **5383 normalised lines conserved
  1-to-1** (`scripts/split-proof-hl-tui.py 50fd36d5`). Normalises away only what a split
  may change: module docs, `use`/`mod` items, the extra `impl HlTuiApp {` wrapper, and
  `pub(super)` visibility bumps.
- `vaughan-tui/src/app.rs::spawn_job` → `jobs/{refresh,send,dex,dca,mcp}.rs`: of 33 moved
  bodies, **29 are token-identical**, 3 differ only in closure-brace placement
  (`|_| { e }` → `|_| e`), and 1 (`dex_quote`) differs only in braces/whitespace/commas
  from a block-arm → expression-arm restyle. Verified at three tiers by
  `scripts/split-proof-app-jobs.py 50fd36d5`; every identifier, literal, operator, call
  and argument order matches in all 33.
- **Argument-forwarding check (the one that matters for funds):** several extracted
  functions take 3-6 consecutive `String` params (`token`, `to`, `amount`), where a
  transposed call site would compile cleanly and send the wrong amount to the wrong
  address. All **30 delegating call sites** forward every pattern-bound field, positionally
  matching the callee's parameter names. No transposition.
- `UiJob` / `UiJobResult` definitions in `jobs/mod.rs` are **untouched** (diff is the module
  doc + 5 `mod` lines); the pre-existing `UiJob::Lp*` arms are byte-identical; every line
  added to `app.rs` is an import change or a delegating-call argument (net −810 lines).

**P2 — `spawn_job`'s stated lock invariant is violated by six jobs (pre-existing).**
`vaughan-tui/src/app.rs:3546` asserts *"Never hold the wallet mutex across RPC / signing
awaits — that freezes the TUI on `[busy]`"*. Six jobs do exactly that, in two shapes:
**(A)** a guard bound and still in scope at `block_on`, and **(B)** the lock taken *inside*
the `async` block that then `.await`s — which the compiler itself flags, hence the
`#[allow(clippy::await_holding_lock)]`:

| Job | Shape | Held across |
|---|---|---|
| `mcp.rs::mcp_queued_approve` | B | `execute_approval_with_fee(..).await` — MCP-approved **sign + broadcast**, needs `&mut WalletState` throughout |
| `send.rs::send_stealth` | A | `w.send_stealth(..)` — a **sign + broadcast** |
| `dex.rs::dex_swap_estimate_after_approve` | A | `w.estimate_transaction_fee(..)` |
| `refresh.rs::refresh_activity` | A | `w.activity(limit)` |
| `refresh.rs::refresh_allowances` | A | `w.list_allowances()` |
| `refresh.rs::replace_broadcast` | A | `w.replace_broadcast(..)` |

Verified pre-existing and unchanged by the split (`git show 50fd36d5:…/app.rs` has the
identical pattern, `#[allow]` included), so this is **latent, not a regression** — but the
two sign+broadcast entries hold the wallet for a full signing round-trip, the worst case
for the freeze the comment warns about. Deliberately **not** fixed here: re-scoping a guard
is a behaviour change and has no place in a move-only commit. Filed in `TASKS.md`.

Note for whoever picks this up: shape B is not simply re-scopable —
`execute_approval_with_fee` takes `&mut WalletState` for the duration by design, so that
one needs an interface change, not just an earlier `drop(w)`. The four read-only shape-A
cases are the cheap wins.

**P2 — module docs asserted a lock discipline the code does not honour (introduced, fixed).**
The `jobs/{refresh,send}.rs` docs written in §7 claimed *every* function drops the wallet
lock before its await, which the table above falsifies. A doc promising a safety property
the code lacks is worse than no doc — the next agent will build on the false premise.
Corrected in the same change: each module doc now names its exceptions explicitly.
`jobs/mod.rs`'s claim that job bodies "live in `app.rs`'s `spawn_job`" was also stale after
the split and now describes reality.

**Not covered**: still no full line-by-line Pass A across `vaughan-core`'s 41k lines; the
splits were verified as splits (token conservation + call-site correctness), which proves
behaviour was *preserved*, not that the pre-existing behaviour is *correct*.
