# Web3 & DEX security — how Vaughan paths compare

Plain-language comparison of **how you interact with DeFi** and what can go wrong.
Use this to pick a path — not as a formal audit certificate.

**Last updated:** 2026-08 (includes **VB** — Vaughan Browser, formerly “VAB”).

---

## Start here: which path should I use?

| Your goal | Best path | Why |
|-----------|-----------|-----|
| Swap, bridge, inspect contracts, MCP agents — **no website** | **Browserless Pulse** (default) | No dApp frontend to hijack; you approve calldata in the TUI |
| Must use a **real dApp website** (PulseX mirror, 9inch, …) | **VB** (`vaughan-dapp-browser`) | Allowlisted Chromium + extension; signing still in TUI |
| Legacy / fallback webview | **Freedom** (parked) | Same general web risks as VB; prefer VB when installed |
| MetaMask, Rabby, WalletConnect, … | *(not Vaughan)* | See “Typical browser wallets” below |

**Rule of thumb:** prefer **Browserless Pulse** whenever it can do the job. Use **VB** only when you need the actual web UI.

Related docs: [browserless-pulse.md](browserless-pulse.md), [dapp-browser-strategy.md](dapp-browser-strategy.md), [dapp-connection-risks.md](dapp-connection-risks.md).

---

## How to read the ratings

Each cell answers: *“If this attack happens, how bad is it for this path?”*

| Rating | Meaning |
|--------|---------|
| **Strong** | Architecture removes the attack or makes it impractical |
| **Good** | Keys stay safe; you must read the TUI prompt (human error still possible) |
| **Careful** | Web or browser involved — review what you sign; some exposure remains |
| **Weak** | Common real-world loss vector for this model |
| **—** | Not applicable (e.g. no web page) |

---

## Vaughan paths at a glance

Side-by-side view of the three ways **Vaughan** can touch a dApp.

| Threat | What it means (one line) | Browserless Pulse | VB | Freedom |
|--------|--------------------------|:-----------------:|:--:|:-------:|
| **Page tricks your wallet** (DOM / JS patching) | Malicious site replaces `window.ethereum` or spies on calls | **Strong** — no page | **Good** — tamper watchdog + sealed provider + TUI truth | **Good** — isolated IPC to signer |
| **Fake or hacked website** (DNS, CDN, mirror) | Frontend shows a legit UI but builds a drainer tx | **Strong** — no frontend | **Careful** — allowlisted hosts; IPFS mirrors seeded | **Careful** — same web trust as VB |
| **Keys stolen from browser memory** | Seed / private key in JS heap | **Strong** — Rust vault only | **Strong** — Rust vault only | **Strong** — Rust vault only |
| **Signing without understanding** (blind sign) | You approve hex you never decoded | **Strong** — ABI decode in TUI / REPL | **Good** — full tx in TUI before sign | **Good** — full tx in TUI before sign |
| **Tracking** (IP, fingerprint, relay logs) | Someone learns who you are and what you hold | **Strong** — direct RPC, no browser | **Good** — reads via Vaughan RPC + privacy flags; sites may still analytics | **Careful** — webview + sites |
| **Infinite approvals / drainers** | One bad signature empties the wallet | **Strong** — explicit actions | **Good** — no auto-sign; Connect + TUI approve | **Good** — no auto-sign; TUI approve |

**Summary**

- **Browserless Pulse** — strongest Vaughan path; default for a reason.
- **VB** — keys/signing as safe as Freedom; **DOM parity ~Good** (watchdog); **tracking ~Good** (RPC proxy); web frontend trust still **Careful**
- **Freedom** — parked fallback; similar web risk to VB, different bridge (Electron IPC vs extension).

---

## Full comparison (including non-Vaughan wallets)

| Threat | MetaMask / extension | WalletConnect v2 | Browserless Pulse | **VB** | Freedom + Vaughan |
|--------|:--------------------:|:----------------:|:-----------------:|:------:|:-----------------:|
| Page tricks your wallet | Weak | — | **Strong** | **Good** | **Good** |
| Fake or hacked website | Weak | Weak | **Strong** | **Careful** | **Careful** |
| Keys in browser JS heap | Weak | Good | **Strong** | **Strong** | **Strong** |
| Blind signing | Weak | Weak | **Strong** | **Good** | **Good** |
| Tracking / relay metadata | Weak | Weak | **Strong** | **Good** | **Careful** |
| Approval drainers | Weak | Weak | **Strong** | **Good** | **Good** |

*Browserless Pulse = Ag / Dex / Browse / MCP / `wiz4rd-engine` — no web engine.*

---

## Threat details (by attack type)

### 1. Page tricks your wallet (DOM tampering)

**What happens:** JavaScript on the page monkey-patches `window.ethereum.request`, shows fake balances, or intercepts RPC.

| Path | Notes |
|------|-------|
| MetaMask / extension | Page and extension share the browser; classic attack surface |
| WalletConnect | — (page talks to phone/desktop wallet via relay) |
| Browserless Pulse | No DOM, no `window.ethereum` |
| **VB** | EIP-1193 inject in MAIN world for dApp interop. **P1 hardening:** sealed provider + 4s tamper watchdog + EIP-6963 re-announce; reads proxied through Vaughan RPC. Sign/send truth is always the TUI |
| Freedom | Electron **isolated world** + IPC; page does not hold the signer |

### 2. Fake or hacked website (DNS, CDN, IPFS mirror)

**What happens:** You think you’re on PulseX / Uniswap; a compromised script builds a transfer to an attacker.

| Path | Notes |
|------|-------|
| MetaMask / WC | Full web stack trust |
| Browserless Pulse | Calls routers/contracts via Rust + RPC; no site to poison |
| **VB** | Loads real HTTPS UIs on an **allowlist**. In-tab navigation gated (MV3). PulseX **IPFS mirror** hosts are seeded |
| Freedom | Same web trust as VB; payload reviewed in TUI |

### 3. Keys stolen from browser memory

**What happens:** Malware or XSS reads the decrypted seed from extension memory.

| Path | Notes |
|------|-------|
| MetaMask | Seed decrypted in extension JS heap — high impact |
| WalletConnect | Keys usually on phone/cold device — lower browser exposure |
| Vaughan paths (all) | Vault: Argon2id + AES-256-GCM; unlock in Rust with zeroization. Browser/extension holds at most a **session WS token**, not the seed |

### 4. Signing without understanding (blind sign)

**What happens:** UI shows “Confirm” but calldata is opaque hex; you approve a drainer.

| Path | Notes |
|------|-------|
| MetaMask / WC | Often truncated or opaque in popup |
| Browserless Pulse | Dynamic ABI decode (`alloy-dyn-abi`); REPL / Ag / Dex show intent |
| **VB / Freedom** | Every sign/send → **Vaughan TUI** with recipient, value, fees; no auto-sign |

### 5. Tracking (IP, fingerprint, relays)

**What happens:** RPC provider, WalletConnect relay, or analytics learns your IP, wallet, and timing.

| Path | Notes |
|------|-------|
| MetaMask | Default RPC + browser fingerprint |
| WalletConnect | Third-party relay sees origin, IP, session metadata |
| Browserless Pulse | Client → your RPC; no WC relay, no browser |
| **VB** | Provider on loopback; **`eth_call` / `eth_estimateGas` / …** forwarded to Vaughan’s active RPC (not the page’s Infura key). Chromium privacy flags (WebRTC, background net). Sites may still run analytics |
| Freedom | Loopback provider; webview may still load third-party assets |

### 6. Infinite approvals & drainers

**What happens:** One `approve(max)` or malicious router call empties balances.

| Path | Notes |
|------|-------|
| MetaMask / WC | One-click approve habits |
| Browserless Pulse | Explicit swap/approve flows in TUI |
| **VB / Freedom** | Connect grant required; **fresh TUI approve** per sign/send; never cached |

---

## Architecture (data flow)

### Typical extension (MetaMask)

```
Untrusted web page ──► Extension (JS heap) ──► Cloud / public RPC
         ▲
   DOM exploits, poisoned CDN, key material in browser
```

### Browserless Pulse (default — strongest)

```
Vaughan TUI (Ag / Dex / Browse / MCP) ──► Rust + alloy ──► RPC node
         ▲
   No browser, no web frontend, no window.ethereum
```

### VB (Vaughan Browser)

```
Allowlisted HTTPS tab (system Chromium)
    │  MAIN inject: window.ethereum (page-visible)
    ▼
MV3 extension (background WebSocket, attested page origin)
    │  ws://127.0.0.1:8745 + access_token
    ▼
vaughan-provider ──► Vaughan TUI approve/deny
    ▲
Keys never leave Rust; nav allowlist blocks non-trusted hosts
```

### Freedom + Vaughan (parked fallback)

```
Electron webview (sandbox) ── isolated IPC ──► Freedom main ── 127.0.0.1 ──► Vaughan Rust
         ▲
Keys in Rust; web UI trust similar to VB
```

---

## VB-specific controls (2026 P0 + P1)

| Control | What it does |
|---------|----------------|
| Host allowlist + in-tab nav gate | Extension blocks main-frame loads outside trusted hosts (+ IPFS gateway seeds) |
| Read RPC proxy | Allowlisted `eth_call`, `eth_estimateGas`, … → Vaughan active network RPC (Freedom-style) |
| Tamper watchdog | Sealed `window.ethereum` + periodic integrity check + EIP-6963 re-announce |
| Isolated temp profile | Not your daily Chrome profile; session dir `0700` |
| Provider `access_token` | Loopback WS requires session token |
| Origin allowlist + attested `vaughan_page_origin` | Extension Origin from Chrome, not page-supplied |
| Connect grant before sign | `eth_sign` / send require prior Connect approval for that origin |
| CDP default off | Agent debugging on loopback only when explicitly enabled; token in `vb.session` |
| Chromium privacy flags | WebRTC IP policy, no background networking, no sync |

See [dapp-browser-strategy.md](dapp-browser-strategy.md) and [dapp-connection-risks.md](dapp-connection-risks.md) for open risks (TUI focus, prompt expiry, etc.).

---

## What this document is not

- Not a substitute for a professional security audit
- Not a guarantee of mainnet safety — **testnet first**, read every TUI prompt
- Not exhaustive of all Vaughan surfaces (MCP, hardware wallets, AA batching have their own docs)
