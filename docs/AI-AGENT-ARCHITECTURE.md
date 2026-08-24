# AI Agent Architecture & Security Sandboxing Specification

**Version**: 1.2 (Unified Pipeline & Multi-RPC Quorum)  
**Status**: Approved Specification  
**Applies to**: `vaughan-agent`, `vaughan-core`, `vaughan-tui`, `vaughan-cli`, `vaughan-provider`

---

## 1. Overview & Core Philosophy

Vaughan AI Agent integration extends the sovereign wallet foundation (`vaughan-core` + `wiz4rd-engine`) with autonomous and semi-autonomous AI intelligence without ever compromising private key security, user privacy, or sovereign self-custody.

The architecture strictly enforces four fundamental axioms:
1. **The AI Agent is an Advisor by default, never a Signer without physical capital isolation.**
2. **Unified Approval Pipeline:** AI agent proposals and dApp EIP-1193 requests pass through the exact same ground-truth verification and confirmation UI.
3. **Multi-RPC Quorum:** Autonomous Degen circuit breakers verify pool reserves across multiple independent RPC endpoints to defeat compromised/out-of-sync nodes.
4. **Sentient profile isolation:** Agent-led execution uses the `sentient`
   profile (the agent’s own seed; legacy name `degen`). Human `default` stays
   separate unless both parties intentionally share a mnemonic (partnership).

---

## 2. The 3-Tier Operating Mode Hierarchy

Operating mode is chosen **once at startup / onboarding** and is **permanently immutable for the lifetime of that process session**.

```
                              STARTUP / WELCOME
                                      │
         ┌────────────────────────────┼────────────────────────────┐
         ▼                            ▼                            ▼
  [1] Pure Human Mode         [2] AI Assist Mode          [3] Sentient Mode
  • Zero AI code initialized  • AI Adviser loaded         • Agent’s own seed
  • Zero LLM network calls    • Propose-Only (Read safe)  • Auto under policy
  • 100% manual wallet        • Unified Approval Pipeline • Multi-RPC Quorum check
  • CANNOT switch to AI       • CANNOT auto-execute       • Hard slippage ceilings
```

### 1. Mode 1: Pure Human Mode (`OperatingMode::HumanOnly`)
* **Structural Enforcement**: The `AgentEngine` struct is `Option::None` in memory. Zero background workers are spawned, zero LLM memory is allocated, and the Agent view and shortcuts are completely omitted from the UI router.
* **Compile-Time Feature Flag**: Vaughan provides an optional `cargo build --no-default-features` path that completely compiles out the `vaughan-agent` crate for pure sovereign cold storage.
* **Session Lock**: Impossible to switch to AI assist during the session.

### 2. Mode 2: AI Assist Mode (`OperatingMode::AiAssisted`) — **Adviser**
* **Purpose**: Active DeFi users seeking contract inspection, arbitrage scanning, transaction simulation, and natural language batch composition.
* **Security Model**: The agent has **zero access to private keys or signing capabilities**.
* **Unified Approval**: Generates a typed `TxProposal` which transforms into a standard `HostRequest::Transaction`. The confirmation screen uses the identical ground-truth bytecode decoder used by the EIP-1193 dApp bridge.

### 3. Mode 3: Sentient Mode (`OperatingMode::DegenTrader` — legacy enum name)
* **Seed ownership**: Profile `sentient` (`~/.vaughan/…/profiles/sentient/`; legacy path `…/degen/`) holds **the agent’s seed**.
* **Partnership**: A human may share that mnemonic to co-hold the same vault; otherwise keep `default` separate.
* **Execution**: Automated signing bound by **circuit breakers**, **Multi-RPC Quorum**, and an emergency kill-switch.
* **MCP**: `vaughan-sentient` / `--profile sentient` (when wired).

---

## 3. Defense-in-Depth Security Sandboxing

```
┌─────────────────────────────────────────────────────────────┐
│                 vaughan-agent (SANDBOX)                     │
│  - CAN: Read balances, inspect contracts, run simulations   │
│  - CAN: Build & propose transactions                        │
│  - CANNOT: Access private keys, mnemonic, or seed phrases   │
│  - CANNOT: Sign transactions or broadcast to RPC directly   │
└──────────────────────────────┬──────────────────────────────┘
                               │ Emits TxProposal (NOT signed)
                               ▼
┌─────────────────────────────────────────────────────────────┐
│              Vaughan Core Invariant Engine                  │
│  - Verifies value limits, slippage bounds, simulation status│
│  - Runs Multi-RPC Quorum checks on pool reserves            │
└──────────────────────────────┬──────────────────────────────┘
                               │ Validated Proposal
                               ▼
┌─────────────────────────────────────────────────────────────┐
│         UNIFIED HUMAN CONFIRMATION GATE (TUI / CLI)         │
│  - Shared with EIP-1193 dApp bridge (Zero UI Drift)         │
│  - Decodes raw on-chain calldata independently of AI text   │
│  - User explicitly presses [Enter] or provides password     │
└──────────────────────────────┬──────────────────────────────┘
                               │ User Approved
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                Vault Signer & Broadcaster                   │
│  - Signs with private key and broadcasts to network         │
└──────────────────────────────┘
```

### Layer 1: Architectural Air-Gap & Memory Isolation
* When the vault is unlocked with the master password, it unseals into two strictly isolated structs:
  1. `SignerContext` (holds derived private keys — passed **only** to the manual approval gate or degen worker).
  2. `AgentConfig` (holds API keys / endpoint configuration — passed to `vaughan-agent`).
* `vaughan-agent` receives **zero memory references** to `SignerContext` or `Vault`.

### Layer 2: Cryptographic Vault Parameters
Vault encryption uses Argon2id + AES-256-GCM with explicit production parameters:
* **Memory Cost ($m$)**: `65,536 KiB` (64 MiB RAM)
* **Time Cost ($t$)**: `3` iterations
* **Parallelism ($p$)**: `4` lanes
* **AEAD Cipher**: AES-256-GCM (96-bit nonce, 128-bit authentication tag)
* **Salt**: 16 bytes CSPRNG (`rand::rngs::OsRng`)

---

## 4. Degen Mode Circuit Breakers & Multi-RPC Quorum

In `DegenTrader` mode, autonomous signing is governed by deterministic limits enforced in Rust:

1. **Multi-RPC Quorum Validation**:
   - Before executing a swap or rebalance, the circuit breaker queries pool reserves and token balances across the **Primary RPC** and **Secondary RPC** in parallel.
   - If values differ by $> 0.5\%$, execution is aborted with an `RpcQuorumMismatch` error to prevent RPC spoofing.
2. **Position Sizing Limit**: No single transaction may exceed `X%` of the active degen wallet balance (default: 20%).
3. **Dual-Horizon Gas Ceilings**:
   - **Rolling Window**: Max `X PLS` gas spent within 10 minutes.
   - **Cumulative Session Budget**: Hard lifetime gas cap (e.g. `Y PLS`). Once reached, the agent enters a mandatory hard stop.
4. **Gas-to-Value Ratio Tripwire**: Automatically rejects any transaction where estimated gas cost exceeds 5% of the transaction's economic value.
5. **Adaptive Slippage Ceiling**: Price impact is calculated against pool reserves using `wiz4rd-engine`. Trades exceeding calculated depth or a 1.0%–2.5% safety wall are blocked.
6. **Emergency Stop (Kill Switch)**: Pressing `Esc` or `q` immediately aborts active agent tasks, cancels pending operations, and locks the session.

---

## 5. Multi-Provider LLM Integration

* **Local / Privacy-First**: Direct HTTP client to local OpenAI-compatible daemons (Ollama at `http://127.0.0.1:11434`, `llama.cpp`, LocalAI). Zero wallet metadata leaves the machine.
* **Cloud Providers**: Google Gemini API, Anthropic, OpenAI. API keys are stored encrypted with AES-256-GCM in the user's Vaughan vault and decrypted only into `AgentConfig`.

---

## 6. User Interface & Bridge Integration

* **Unified Approval Modal** (`vaughan-tui/src/views/approval.rs`): Shared confirmation view for both EIP-1193 dApp requests and AI agent proposals.
* **TUI Agent View** (`vaughan-tui/src/views/agent.rs`): Interactive chat interface with real-time streaming tokens, tool invocation logs, and quick action shortcuts.
* **Welcome Screen 3-Way Selector**: Initial onboarding and startup view allowing explicit selection between `[1] Pure Human`, `[2] AI Assisted`, and `[3] Degen Bot`.
* **CLI Agent** (`vaughan agent "<prompt>"`): Non-interactive execution for automated terminal workflows, scripts, and CI/CD pipelines.
