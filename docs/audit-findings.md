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

- [ ] **Fix Global `'q'`**: Restrict single-character `'q'` quit handling to non-input views; use `Ctrl+C`/`Ctrl+Q` globally.
- [ ] **Fix `Tab` Key Flow**: Delegate `Tab` to view inputs before cycling screens.
- [ ] **Non-blocking TUI**: Replace `handle.block_on` in TUI views with async task spawning and channel messages.
- [ ] **Set POSIX `0o600` Permissions**: Restrict `wallet.json` and directory permissions on Unix.
- [ ] **Remove Nonce Caching**: Query `BlockNumberOrTag::Pending` directly during transaction creation.
- [ ] **Optimize HD Derivation**: Cache master/parent derivation to eliminate redundant PBKDF2 iterations.
- [ ] **Support `personal_sign` Param Order**: Auto-detect `[data, address]` vs `[address, data]`.
- [ ] **Enhance `Input` Widget**: Add cursor navigation (`Left`/`Right`/`Delete`/`Home`/`End`).
- [ ] **Terminal Panic Hook**: Ensure clean terminal restore on unexpected panics.
- [ ] **Wire `EventBus` to Settings**: Emit `ChainChanged` on network change.
- [ ] **Gas Precedence**: Only set `gas_price` if `max_fee_per_gas` is absent.
