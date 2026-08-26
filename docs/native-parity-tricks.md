# Achieving Native Security Parity Without Forking Freedom Browser

**Document Status**: Security Architecture Specification  
**Goal**: Elevate Vaughan + Freedom Browser loopback IPC to achieve complete security parity with (and in key areas surpass) a built-in C++ native engine wallet without maintaining a browser fork.

---

## 1. Context & Objective

In traditional browser wallet architectures, a "Native Provider" historically referred to a C++ engine-level provider embedded directly in the browser's DOM/rendering engine.

Because Freedom Browser is an Electron-based browser with a modular main-process `Signer` dispatch architecture (`src/main/wallet/signers.js`), we can connect Vaughan over local loopback (`127.0.0.1:8745` or Unix socket) as a clean upstream PR (MPL-2.0).

To ensure this model has **zero security gaps** compared to an embedded C++ provider, we implement **six architectural hardening techniques ("tricks")**.

---

## 2. The 6 Hardening Techniques

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Freedom Browser (Webview + Main Process)                 │
│    • Injected `window.ethereum` shim (Isolated World)       │
│    • Electron IPC routes sign requests to `signer.js`       │
│    • Signs origin & payload with Shared Session Secret      │
└──────────────────────────────┬──────────────────────────────┘
                               │ Authenticated IPC (Token / HMAC / Unix Sock)
┌──────────────────────────────▼──────────────────────────────┐
│ 2. Vaughan-CLI Daemon (Rust Enclave)                        │
│    • Validates Handshake Token & Origin HMAC                │
│    • Decodes ABI Calldata directly in native Rust           │
│    • Renders un-tamperable terminal TUI prompt              │
│    • Signs with zeroized key material upon human approval   │
└─────────────────────────────────────────────────────────────┘
```

---

### Trick 1: Shared Ephemeral Session Secret (App-Token Handshake)

#### Problem
Loopback ports (`127.0.0.1:8745`) can theoretically be probed by external web browsers (e.g. Chrome, Firefox) visiting hostile web pages that initiate cross-origin WebSockets, or by local user-space scripts.

#### Implementation
1. On startup, Vaughan writes a cryptographically secure, random 256-bit token to `~/.vaughan/session.token` with strict POSIX permissions (`0o600` / `rw-------` on Unix, restricted ACL on Windows).
2. Freedom Browser’s main process (`src/main/wallet/vaughan/transport.js`) reads `~/.vaughan/session.token` on launch.
3. Freedom Browser attaches the token as a WebSocket handshake header:
   ```http
   GET / HTTP/1.1
   Host: 127.0.0.1:8745
   Upgrade: websocket
   Connection: Upgrade
   Authorization: Bearer <session_token>
   ```
4. `vaughan-provider` validates the header in `server.rs`. Any connection lacking a valid session token is dropped before reaching the JSON-RPC layer.

#### Security Outcome
Unauthorized browsers, background daemons, or local malware are **100% blocked** from connecting or querying `eth_accounts`.

---

### Trick 2: Cryptographic dApp Origin Attestation (Anti-Spoofing HMAC)

#### Problem
In traditional Web3 setups, a malicious script or iframe could theoretically try to forge or misrepresent the originating dApp domain.

#### Implementation
1. For every signing request, Freedom Browser’s main process attaches origin metadata and signs the entire JSON payload using HMAC-SHA256 with the shared session secret:
   ```json
   {
     "jsonrpc": "2.0",
     "id": 42,
     "method": "eth_sendTransaction",
     "params": [{ ... }],
     "meta": {
       "origin": "https://app.uniswap.org",
       "tabId": 3,
       "timestamp": 1723758000
     },
     "hmac": "9f83ab... [HMAC of payload + origin]"
   }
   ```
2. Vaughan verifies the HMAC against its local session secret before rendering the prompt.
3. Vaughan displays the cryptographically verified origin prominently:
   `Origin: https://app.uniswap.org [Verified by Freedom Browser Tab #3]`

#### Security Outcome
Fake events, rogue iframes, and origin spoofing become **mathematically impossible**.

---

### Trick 3: Port-Snatching Defense (Handshake Challenge-Response)

#### Problem
If a malicious local script starts before Vaughan and binds `127.0.0.1:8745`, it could attempt to impersonate Vaughan and log outgoing dApp transaction payloads.

#### Implementation
1. Immediately upon establishing a WebSocket connection, Freedom Browser issues an internal handshake challenge:
   ```json
   { "jsonrpc": "2.0", "id": 1, "method": "vaughan_handshake", "params": { "nonce": "random_nonce_hex" } }
   ```
2. Vaughan hashes the nonce with the shared session secret and returns the signature proof.
3. If an imposter service is running on port 8745, the handshake fails; Freedom Browser immediately severs the connection and displays a security alert.

#### Security Outcome
Guarantees Freedom Browser is strictly communicating with the legitimate `vaughan-cli` instance.

---

### Trick 4: Out-of-Band ABI Calldata Decoding (WYSIWYS — Anti-Clickjacking)

#### Problem
In-browser wallet popups (MetaMask, Rabby) render approval prompts inside the browser DOM, making them vulnerable to DOM clickjacking, CSS overlay exploits, font substitutions, or invisible unicode homoglyphs.

#### Implementation
1. `vaughan-core` implements native ABI decoding for common protocols (ERC-20, ERC-721, Uniswap V2/V3, Multicall, Ambire SCW).
2. When an `eth_sendTransaction` or `vaughan_signTransaction` request arrives, Vaughan parses the raw `data` hex and prints the exact contract method, parameters, and recipient cleanly in the terminal frame:
   ```
   ┌─────────────────────────────────────────────────────────────┐
   │ ACTION:   ERC-20 Approve                                    │
   │ TOKEN:    USDC (0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48) │
   │ SPENDER:  Uniswap V3 Router                                 │
   │ AMOUNT:   500.00 USDC                                       │
   │ NETWORK:  PulseChain Mainnet (Chain 369)                    │
   └─────────────────────────────────────────────────────────────┘
   ```
3. The prompt is rendered directly via crossterm on the OS terminal buffer, completely outside Chromium’s rendering engine.

#### Security Outcome
Zero susceptibility to browser-level visual tampering or clickjacking.

---

### Trick 5: Strict Account-Binding Guardrail (Wrong-Account Protection)

#### Problem
A dApp page connected to Account #1 could attempt to silently request a signature for Account #2 if the wallet switched active accounts.

#### Implementation
1. Freedom Browser’s `signer.js` specifies `targetAddress: record.address` on all signing calls.
2. Vaughan enforces a strict match:
   - If `targetAddress == wallet.active_address()`, proceed to user prompt.
   - If `targetAddress != wallet.active_address()`, Vaughan halts execution and displays an explicit mismatch warning:
     `"⚠️ Account Mismatch: dApp requested signature for Account 1 (0xabc), but active account is Account 2 (0xdef). Reject or Switch?"`

#### Security Outcome
Prevents dApps from signing against unintended or inactive accounts.

---

### Trick 6: Unix Domain Sockets (Kernel-Level IPC on Linux/macOS)

#### Problem
TCP loopback ports are accessible to all network-enabled software on the operating system.

#### Implementation
1. On Linux and macOS, Vaughan optionally binds a **Unix Domain Socket** instead of a TCP port:
   `~/.vaughan/vaughan.sock` with `0o600` permissions.
2. In Freedom Browser (`transport.js`), the `ws` package connects directly to the Unix socket via a custom `net.createConnection({ path: '~/.vaughan/vaughan.sock' })` agent.

#### Security Outcome
The OS kernel itself restricts socket access strictly to processes owned by the current logged-in user, eliminating network stack exposure entirely.

---

## 3. Comprehensive Threat Matrix

| Threat Vector | Injected Extension (MetaMask) | Hypothetical Built-In C++ Engine | **Vaughan + Freedom (With 6 Tricks)** |
|---|---|---|---|
| **Private Key Extraction via Browser RCE** | ⚠️ Vulnerable (Keys in browser heap) | ⚠️ Vulnerable (Keys in browser heap) | **🛡️ Immune (Keys in separate Rust process)** |
| **DOM Monkey-Patching / Content Script Hijacking** | ⚠️ Vulnerable | 🟢 Protected | **🛡️ Immune (Isolated world + separate IPC)** |
| **Localhost Port Sniffing / S Powers** | ⚠️ N/A | 🟢 Protected | **🛡️ Immune (Session Token + Unix Socket)** |
| **Origin Spoofing / Fake Provider Events** | ⚠️ Possible | 🟢 Protected | **🛡️ Immune (HMAC Origin Attestation)** |
| **Phishing / DOM Clickjacking** | ⚠️ Vulnerable | ⚠️ Vulnerable | **🛡️ Immune (Terminal TUI WYSIWYS Calldata Parsing)** |
| **Code Maintenance Overhead** | Low | ❌ Massive (Forking Chromium) | **🟢 Zero Forking (Clean Upstream PR)** |

---

## 5. Vaughan dApp-browser status (Phase 1 hardening)

Shipped toward Freedom-parity for the Chromium shell (2026-08):

| Trick / control | Status |
|---|---|
| 1 Session token (`provider.session` 0o600) | **Yes** — required for `chrome-extension://` WS; Freedom Origin still Origin-only unless `VAUGHAN_PROVIDER_REQUIRE_TOKEN=1` |
| Page origin on RPC (`vaughan_page_origin`) | **Yes** — derived by the service worker from Chrome-attested `port.sender.url` (page-supplied values are ignored); shown on approve; used as connect-grant key |
| Response routing | **Yes** — service worker assigns JSON-RPC wire ids and maps them back per tab (no cross-tab id collisions / response theft) |
| Token hygiene | **Yes** — token redacted from launcher stderr; extension bundle dir is 0700 under /tmp |
| Approve UI sanitization | **Yes** — control chars stripped from origin/site/message text (no terminal escape injection) |
| Connect grant (`eth_accounts` empty until `eth_requestAccounts` approved) | **Yes** — per unlock session; sign/send prompts on the extension path require a prior grant (4100 otherwise) |
| `accountsChanged` scoping | **Yes** — service worker forwards account events only to origins that hold a grant (learned from non-empty accounts results); cleared on lock / WS close |
| Locked wallet | **Yes** — `eth_requestAccounts` returns 4100 (no silent hang) while locked |
| Approve debounce (400ms) + 60s auto-deny | **Yes** |
| `wallet_switchEthereumChain` prompts | **Yes** — prompt shows requesting origin |
| `tx.from` must match active account | **Yes** |
| 2 HMAC origin attestation | Not yet (needs Freedom transport PR) |
| 3 Handshake challenge | Not yet |
| 6 Unix domain socket | Not yet |
| Token required for https origins (Freedom) | Opt-in via `VAUGHAN_PROVIDER_REQUIRE_TOKEN=1`; default flips after Freedom reads `provider.session` |
| In-tab navigation allowlist | Still Phase 1 gap |

Extension attaches `?access_token=` automatically from `provider.session`.
Freedom PR should read the same file and send `Authorization: Bearer`.
