# Freedom Browser Integration — Research & Plan

> Status: research complete (verified against `solardev-xyz/freedom-browser`, `main` branch, July 2026).
> Companion to `TASKS.md` Phase 2 and `REQUIREMENTS.md` FR-2.x.
> License of Freedom Browser: **MPL-2.0** — our integration is an **upstream PR**, not a fork.

---

## 1. What Freedom Browser is

Freedom Browser is an **Electron application** (`"main": "src/main/index.js"`, `electron ^43`),
not a WebKitGTK/engine-level browser. Protocol logic lives in the **main process**; pages render
in webviews; the renderer talks to main over **Electron IPC** (channels in
`src/shared/ipc-channels.js`).

This matters: there is **no C++ engine layer** to modify. Any brief describing IDL files or a
`JSEthereumProvider.cpp` is about a different browser and does not apply here.

---

## 2. Its wallet architecture (verified findings)

### 2.1 The `src/main/wallet/` module

| File | Role |
|---|---|
| `signers.js` | **Signer factory** — resolves a wallet index to a signer backend by account `type` |
| `ledger/` | Ledger backend — **the template for our Vaughan backend** (`signer.js`, `transport.js`, `errors.js`, `ipc.js`) |
| `provider-manager.js` | ethers.js RPC providers (read calls; fallback pools) — *not* the injection |
| `rpc-manager.js` | User-configured RPC API keys (Alchemy/Infura/DRPC) |
| `dapp-permissions.js` | Per-origin dApp permissions (which wallet index may sign for which origin) |
| `wallet-ipc.js` | IPC handlers for wallet operations (send, estimate gas, …) |
| `transaction-service.js` | Tx building/population (nonce, fees) + broadcast |
| `balance-service.js`, `tx-recorder.js`, `vault-access.js`, `chains.js` | Balance, tx history, key borrowing, chain catalog |

`identity-manager.js` (one level up) owns the vault and the wallet list:

- `WALLET_TYPES = { MNEMONIC: 'mnemonic', LEDGER: 'ledger' }` → we add `VAUGHAN`.
- `addLedgerWallet(name, address, path)` → we mirror with `addVaughanWallet(name, address)`.
- Hardware accounts live at index ≥ `HARDWARE_INDEX_BASE` (1,000,000) so dApp permissions /
  publisher identities pinned to an index can never rebind to a different account. Vaughan
  accounts should follow the same disjoint-index discipline.

### 2.2 The `Signer` contract we implement (from `signers.js`)

```js
Signer {
  getAddress()      -> Promise<string>  // checksummed address (cached after first resolve)
  signTransaction(tx) -> Promise<string>// complete unsigned tx (nonce, gas, fees, chainId) → serialized signed tx
  signMessage(message) -> Promise<string>   // EIP-191 signature over raw bytes
  signTypedData(typedData) -> Promise<string> // EIP-712; full payload {domain, types, message}
}
```

Input normalization happens once in the factory (0x-hex messages → raw bytes; JSON-string
typed data → object). Backends receive the same shapes.

Factory dispatch (today):

```js
const backend = record && record.type === WALLET_TYPES.LEDGER
  ? createLedgerBackend(record)
  : createVaultBackend(walletIndex);
```

We insert `createVaughanBackend(record)` for `record.type === WALLET_TYPES.VAUGHAN`.

### 2.3 The Ledger backend — behaviors we copy

- **`getAddress`** serves the address stored on the account record — no round-trip.
- **Signing verifies identity first**: before signing, the attached device must derive the
  account's stored address, else `WRONG_DEVICE` error ("signing with a different device/seed
  must fail, not silently produce a foreign signature"). → Vaughan equivalent: verify the
  server's `eth_accounts` matches `record.address` before signing.
- **Stable error codes** (`LEDGER_ERROR_CODES`) with **user rejection distinct** from failure
  (EIP-1193 4001) so approval UIs can tell "declined" from "broken".
- **No hosted services**: Ledger's clear-signing lookups (token/EIP-712 metadata registries)
  are deliberately disabled — "no user data leaves the machine to sign". Vaughan must hold
  the same property: everything stays local.
- **Transport**: per-operation open/close, serialized queue, lazy-loaded libs.

### 2.4 dApp-facing injection (page side — already exists)

- IPC channel `internal:get-ethereum-inject-source` — main serves the Ethereum injection
  source to webview preloads.
- `PRIVATE_IS_PRIVATE` comment: *"webview preloads ask whether they run inside a private
  window before injecting the wallet providers"*.
- `rpc-manager.js`: *"The injected dApp provider resolves a chain's RPC pool through this"*.

So the page-facing half is the browser's existing machinery (preload → isolated world →
IPC). We do **not** build page-level injection; we build the **main-process signer backend**
it funnels into.

### 2.5 Dependencies (no new browser deps needed)

`package.json` already has `ethers ^6.16` and `ws ^8.21` (override, pulled in by ethers).
A WebSocket client to our local server needs nothing new.

---

## 3. How Vaughan connects — the architecture

```
dApp page (webview)
  └─ window.ethereum.request({method, params})     ← injected provider (browser preload, isolated world — exists)
       └─ Electron IPC                              ← browser's wallet-ipc channels (exists)
            └─ signers.js → VAUGHAN backend         ← NEW, upstream PR (mirrors ledger/)
                 └─ WebSocket  ws://127.0.0.1:8745  ← EIP-1193 JSON-RPC over text frames
                      └─ vaughan-provider server    ← this repo (Rust), loopback-only
                           └─ TUI approval prompt   ← FR-2.3 (every sign/send)
                                └─ vaughan-core signs / broadcasts
```

Ownership:

| Layer | Owner | Status |
|---|---|---|
| Injected `window.ethereum` + IPC | Freedom Browser | exists |
| Vaughan signer backend (`signers.js` dispatch) | **upstream PR** (FR-2.5) | to build |
| EIP-1193 WebSocket server, loopback | `vaughan-provider` (Rust) | FR-2.1 ✅ done |
| EIP-1193 methods (7 + `vaughan_signTransaction`) | `vaughan-provider` | FR-2.2 ✅ done |
| Approval prompts | `vaughan-tui` | FR-2.3 |
| Trusted-host gate | `vaughan-provider` | FR-2.4 |
| `accountsChanged` / `chainChanged` events | `vaughan-provider` | FR-2.2 ✅ done (`EventBus`) |

---

## 4. Protocol contract (EIP-1193 over WebSocket)

- Endpoint: `ws://127.0.0.1:8745` (`vaughan_provider::server::DEFAULT_PORT`).
- JSON-RPC 2.0, one request/response per text frame; binary frames rejected;
  4 MiB frame cap; ping/pong handled.
- Errors: EIP-1193 codes — 4001 user rejected, 4100 unauthorized, 4200 unsupported method,
  4900 disconnected, 4901 chain disconnected; JSON-RPC -32700/-32600/-32602/-32603.

Method ↔ Signer mapping:

| EIP-1193 method | Signer interface | Notes |
|---|---|---|
| `eth_accounts` | `getAddress` | active account; empty when locked |
| `eth_requestAccounts` | `getAddress` | connect gesture; may surface a TUI prompt |
| `eth_chainId` | — | active chain as `0x` hex |
| `eth_sendTransaction` | `signTransaction` | **open decision ↓** |
| `personal_sign` | `signMessage` | params: `[message, address]` |
| `eth_signTypedData_v4` | `signTypedData` | params: `[address, typedData]`; full wire payload |
| `wallet_switchEthereumChain` | — | params: `[{chainId}]`; only built-in networks |

**Decided — broadcast vs sign-and-return.** The browser's `Signer.signTransaction`
contract returns a *serialized signed tx* (the browser populates nonce/fees and broadcasts
via its own RPC pool), so `vaughan_signTransaction` (sign-and-return raw signed tx) is
implemented on the server for the browser backend, while `eth_sendTransaction` (broadcast)
stays for direct dApp/launcher flows.

**Trusted hosts (FR-2.4).** Because the browser already gates dApps per-origin, our server
only needs to trust the *connection itself*. Planned mechanism: require an `Origin` header
(or app token) that only the Freedom Browser backend sends; reject everything else. The
server already captures `Origin` during the WS handshake.

---

## 5. Status of our side (`vaughan-provider`, this repo)

- FR-2.1 ✅ — loopback-only WS server, JSON-RPC framing, `RequestHandler` trait,
  `RequestCtx { peer, origin }`, tests incl. real-socket roundtrips, fmt/clippy clean.
- FR-2.2 ✅ — all 8 methods (`eth_accounts`, `eth_requestAccounts`, `eth_chainId`,
  `eth_sendTransaction`, `personal_sign`, `eth_signTypedData_v4`, `wallet_switchEthereumChain`,
  `vaughan_signTransaction`) via `Eip1193Handler` + `WalletHandle` trait; hex→decimal
  quantity normalization; 4902 for unknown chains.
- FR-2.2 events ✅ — `EventBus` → `accountsChanged`/`chainChanged` relayed as JSON-RPC
  notifications to all connected clients.
- FR-2.3 ✅ — approval flow: TUI implements `WalletHandle` (`ProviderHost`), shows a
  full-screen approve/deny prompt for every sign/send. The provider server auto-starts on
  app launch (loopback `8745`); a bind failure is non-fatal. Every provider request funnels
  through the UI thread via an MPSC channel + `oneshot` reply, so key material never leaves
  the UI thread and nothing signs without the prompt. Core signing: EIP-191
  `personal_sign`, EIP-712 `eth_signTypedData_v4`, raw `vaughan_signTransaction`, and
  general `send_transaction`.
  *Known gap: the prompt shows recipient/value/chain/data but not the fee (estimated at
  execution) — TODO in `vaughan-tui/src/provider.rs`.*
- FR-2.4 — trusted-host gate (Origin/app token).
- Dapps launcher (user vision): TUI screen lists dApp URLs → launches Freedom Browser with
  the provider ready.

---

## 6. Browser-side implementation plan (agent brief — paste to the browser agent)

1. `src/main/wallet/vaughan/transport.js` — WebSocket client to `ws://127.0.0.1:8745`
   (reuse `ws`, already in deps). Per-operation connect, serialized queue, JSON-RPC id
   correlation — mirror `ledger/transport.js`.
2. `src/main/wallet/vaughan/signer.js` — `createVaughanBackend(record)` implementing the
   `Signer` interface. Before signing, verify server `eth_accounts` matches `record.address`
   (mirrors Ledger's wrong-device check). Map: `getAddress → eth_accounts`,
   `signTransaction → vaughan_signTransaction` (or `eth_sendTransaction` — see §4),
   `signMessage → personal_sign`, `signTypedData → eth_signTypedData_v4`.
3. `src/main/wallet/vaughan/errors.js` — `VAUGHAN_*` codes mapping EIP-1193 codes
   (4001 user-rejected, 4100 unauthorized, 4900/4901 disconnected) — mirror `ledger/errors.js`.
4. `src/main/wallet/vaughan/ipc.js` — connect / discover-accounts IPC like `ledger/ipc.js`.
5. `identity-manager.js` — `WALLET_TYPES.VAUGHAN` + `addVaughanWallet(name, address)`
   (address from Vaughan's `eth_requestAccounts`), mirroring `addLedgerWallet`.
6. `signers.js` — dispatch `WALLET_TYPES.VAUGHAN → createVaughanBackend(record)`.
7. Keep the no-hosted-services rule: every signing path stays local to the machine.

---

## 7. What remains to verify in the browser repo

These are inferred from channel names/comments, not yet read:

- `src/main/wallet/wallet-ipc.js` — exact IPC surface the injected provider uses
  (channels seen: `WALLET_ESTIMATE_GAS`, `WALLET_GET_GAS_PRICE`, `WALLET_SEND_TRANSACTION`,
  `WALLET_BUILD_ERC20_DATA`, `WALLET_PARSE_AMOUNT`, `WALLET_GET_TRANSACTION_STATUS`…).
- The actual preload/injection source behind `internal:get-ethereum-inject-source`
  (is it a full EIP-1193 shim? EIP-6963 announce?).
- `src/main/wallet/ledger/ipc.js` — account-discovery UI flow to mirror.
- `src/main/wallet/dapp-permissions.js` — how origin → wallet-index permissions gate signing
  (affects whether Vaughan accounts need browser-side permission wiring).
- How to launch Freedom Browser with a URL (for the Dapps launcher): argv? custom protocol?
  (Electron apps usually accept `electron <url>` or a deep link — confirm in `src/main/index.js`).

## 8. Decisions & corrections (for the record)

1. **No WebKitGTK.** Freedom Browser is Electron; no engine C++, no IDL files.
2. **No fork.** Integration is an upstream PR (MPL-2.0); worst case, a small patch set in
   `src/main/wallet/` — never a hard fork.
3. **Port 8745** is the agreed default (`DEFAULT_PORT`), matching the browser backend.
4. **Injected provider ≠ what we build.** The injected `window.ethereum` is the browser's
   existing front half; we build the native signing endpoint it connects to.
5. **Signing approval lives in Vaughan** (TUI), never in the page and never auto-signed.
