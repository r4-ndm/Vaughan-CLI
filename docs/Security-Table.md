# Web3 & DEX Security Architecture Comparison

A comparative security analysis across four interaction models:
1. **Injected Extension** (MetaMask / Chrome)
2. **Cloud Relay** (WalletConnect v2)
3. **Freedom Browser + Vaughan Backend** (Webview + OS-level Rust Signer)
4. **CLI DEX Engine / `wiz4rd-engine`** (Direct Pure-Rust Terminal Browser)

---

## 1. Quick Comparison Matrix

| Threat / Attack Vector | Injected Extension | Cloud Relay | Freedom + Vaughan | `wiz4rd-engine` (CLI) |
|---|:---:|:---:|:---:|:---:|
| **1. DOM / Monkey-Patching** | ❌ Vulnerable | N/A | 🛡️ **Immune** | 🛡️ **Immune** |
| **2. Malicious Web Frontend / DNS** | ❌ High Risk | ❌ High Risk | ⚠️ Moderate | 🛡️ **Immune** |
| **3. Private Key Extraction (JS Heap)** | ❌ High Risk | 🛡️ Low | 🛡️ **Immune** | 🛡️ **Immune** |
| **4. Blind Signing / Calldata Tampering** | ❌ High Risk | ❌ High Risk | 🛡️ Protected | 🛡️ **Immune** |
| **5. IP / Metadata Tracking** | ❌ High | ❌ Critical | 🛡️ **None** | 🛡️ **None** |
| **6. Unlimited Approval Drainers** | ❌ High Risk | ❌ High Risk | 🛡️ Low Risk | 🛡️ **Immune** |

**Legend**:
- 🛡️ **Immune / Protected**: Architecture mathematically or physically eliminates the attack surface.
- ⚠️ **Moderate / Low Risk**: Attack payload visible in terminal for inspection before signing.
- ❌ **Vulnerable / High Risk**: Common vector for user fund loss and key exposure.

---

## 2. Threat Vector Breakdown

### 1. DOM Tampering & Prototype Pollution
- **Injected Extension**: Hostile scripts on a webpage can monkey-patch `window.ethereum.request` or read unencrypted memory.
- **Freedom + Vaughan**: Protected by Electron isolated-world IPC; keys never touch the web page.
- **`wiz4rd-engine` (CLI)**: **100% Immune.** There is no DOM, no JavaScript runtime, and no browser.

### 2. Compromised Frontends, DNS Hijacks & Malicious CDNs
- **Injected Extension**: If a DEX frontend (e.g. Uniswap/PulseX) is hijacked via DNS or poisoned CDN script, it silently feeds a drainer transaction to the wallet popup.
- **Freedom + Vaughan**: Payload is forwarded to the terminal for review, but relies on user auditing calldata.
- **`wiz4rd-engine` (CLI)**: **100% Immune.** Connects directly to smart contracts over RPC. No web frontends exist to be compromised.

### 3. Private Key & Memory Security
- **Injected Extension**: Seed phrases decrypted into the browser's JavaScript V8 heap.
- **Freedom + Vaughan**: Keys reside exclusively in a separate native Rust process.
- **`wiz4rd-engine` (CLI)**: **100% Immune.** Vault is encrypted with Argon2id + AES-256-GCM; unlocked keys exist in memory-safe Rust with automatic zeroization on drop.

### 4. Calldata Transparency & Blind Signing
- **Injected Extension**: Extension popups frequently display opaque hex payloads or truncated parameters.
- **Freedom + Vaughan**: Calldata decoded in the TUI terminal with fee estimation before prompt.
- **`wiz4rd-engine` (CLI)**: Full dynamic ABI decode via `alloy-dyn-abi`. The user explicitly selects function parameters directly in the CLI/TUI.

### 5. Metadata Privacy & Cloud Relay Tracking
- **Injected Extension**: Default RPC endpoints log browser fingerprints, IPs, and wallet addresses.
- **Cloud Relay (WalletConnect)**: Bridges route all traffic through third-party cloud servers that log origin, IP, and timestamps.
- **Freedom + Vaughan**: Operates exclusively over local loopback (`127.0.0.1:8745`).
- **`wiz4rd-engine` (CLI)**: Direct client-to-node RPC; zero relays, zero IPC overhead.

---

## 3. Architecture Diagrams

#### Traditional Extension Model (MetaMask)
```
[ Untrusted Web Page ] ──► [ Extension (JS V8 Heap) ] ──► [ Cloud RPC ]
▲ Massive attack surface: DOM exploits, malicious CDNs, JS memory extraction.
```

#### Freedom Browser + Vaughan Model
```
[ Webview Sandbox ] ──(Isolated IPC)──► [ Main Process ] ──(127.0.0.1)──► [ Vaughan Rust Core ]
▲ Key isolation: Keys never touch the browser; signed via local terminal approval.
```

#### CLI DEX Engine (`wiz4rd-engine`)
```
[ Terminal CLI / REPL ] ──(Pure Rust alloy-dyn-abi)─────────────────────► [ Direct RPC Node ]
▲ Cold-grade security: Zero browser, zero web frontend, zero third-party relays.
```


