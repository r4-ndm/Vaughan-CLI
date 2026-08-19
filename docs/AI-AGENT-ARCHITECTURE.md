# AI Agent Architecture & Security Sandboxing Specification

**Version**: 1.0  
**Status**: Approved Specification  
**Applies to**: `vaughan-agent`, `vaughan-core`, `vaughan-tui`, `vaughan-cli`

---

## 1. Overview & Vision

Vaughan AI Agent integration extends the sovereign wallet foundation (`vaughan-core` + `wiz4rd-engine`) with autonomous and semi-autonomous AI intelligence without ever compromising private key security, user privacy, or sovereign self-custody.

The architecture strictly follows the fundamental axiom:
> **The AI Agent is an Advisor by default, never a Signer without physical capital isolation.**

---

## 2. The 3-Tier Operating Mode Hierarchy

Operating mode is chosen **once at startup / onboarding** and is **permanently immutable for the lifetime of that process session**. It is impossible to toggle between modes mid-session.

```
                              STARTUP / WELCOME
                                      │
         ┌────────────────────────────┼────────────────────────────┐
         ▼                            ▼                            ▼
  [1] Pure Human Mode         [2] AI Assist Mode          [3] Degen Bot Mode
  • Zero AI code loaded       • AI Advisor loaded         • Autonomous Agent
  • Zero LLM network calls    • Propose-Only (Read safe)  • Isolated burner wallet
  • 100% manual wallet        • Human MUST approve tx     • Circuit breakers active
  • CANNOT switch to AI       • CANNOT auto-execute       • Hard spending caps
```

### 1. Mode 1: Pure Human Mode (`OperatingMode::HumanOnly`)
* **Purpose**: Cold-storage, security purists, and air-gapped workflows.
* **AI Subsystem**: Completely uninitialized / zero AI memory footprint.
* **Network**: Zero calls to external AI APIs or local LLM daemons.
* **Switching**: Impossible to switch to AI assist during this session.

### 2. Mode 2: AI Assist Mode (`OperatingMode::AiAssisted`)
* **Purpose**: Active DeFi users seeking contract inspection, arbitrage scanning, transaction simulation, and natural language batch composition.
* **AI Subsystem**: Loaded in **Advisor (Propose-Only)** mode.
* **Security Model**: The agent has **zero access to private keys or signing capabilities**.
* **Execution**: Autonomous for read queries (balances, contracts, DEX reserves); **strictly requires manual human confirmation (Enter/PIN)** for any transaction proposal.

### 3. Mode 3: Degen Bot Mode (`OperatingMode::DegenTrader`)
* **Purpose**: Autonomous algorithmic and LLM trading on decentralized exchanges (PulseX, Uniswap).
* **Storage Isolation**: Runs in a **dedicated isolated profile directory** (`~/.vaughan/profiles/degen/`) with its own independent seed phrase and funds.
* **Safety**: Primary savings/vault are physically unlinked and inaccessible.
* **Execution**: Automated signing strictly bound by hardcoded Rust **circuit breakers** and an emergency kill-switch.

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
└──────────────────────────────┬──────────────────────────────┘
                               │ Validated Proposal
                               ▼
┌─────────────────────────────────────────────────────────────┐
│              Human Confirmation Gate (TUI / CLI)            │
│  - Displays raw on-chain effects (independent of AI prompt) │
│  - User explicitly presses [Enter] or provides password     │
└──────────────────────────────┬──────────────────────────────┘
                               │ User Approved
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                Vault Signer & Broadcaster                   │
│  - Signs with private key and broadcasts to network         │
└─────────────────────────────────────────────────────────────┘
```

### Layer 1: Architectural Air-Gap
The `vaughan-agent` crate only takes a `ReadOnlyProvider` and `ProposalSink`. It does not hold references to `Vault`, `PrivateKeySigner`, or `Keystore`. Signing is physically performed only by the outer application runtime after human approval or circuit breaker validation.

### Layer 2: Tool Partitioning
* **Autonomous Read Tools**:
  - `get_balance(account, token)`
  - `inspect_contract(target)` (runs `wiz4rd-engine` prober and selector extraction)
  - `get_dex_reserves(pair)`
  - `simulate_call(to, data, value)` (runs pre-flight `eth_call`)
  - `search_pairs(factory, limit)`
* **Propose-Only Write Tools**:
  - `propose_transfer(recipient, amount)`
  - `propose_swap(token_in, token_out, amount_in, min_out, dex)`
  - `propose_batch(calls)` (uses `vaughan-aa` EIP-7702 batch composer)
  - `propose_contract_call(target, function_name, args)`

### Layer 3: Anti-Prompt Injection & Data Sanitization
* **Untrusted Input Isolation**: On-chain data (contract names, token symbols, event logs, revert messages) are treated as untrusted user input and enclosed in structured JSON schemas.
* **Ground-Truth UI Rendering**: The TUI confirmation modal decodes calldata and displays recipient addresses, amounts, and fees directly from the compiled transaction bytecode, ignoring any persuasive text generated by the LLM.

---

## 4. Degen Mode Circuit Breakers

In `DegenTrader` mode, autonomous signing is permitted only within deterministic limits enforced in Rust:

1. **Position Sizing Limit**: No single transaction may exceed `X%` of the active degen wallet balance (default: 20%).
2. **Gas Burn Ceiling**: If cumulative gas spent within 10 minutes exceeds the configured threshold, or if 3 transactions fail consecutively, the agent halts immediately.
3. **Hard Slippage Ceiling**: Swaps with expected slippage > 1.0% are strictly rejected to eliminate MEV sandwich attack risks.
4. **Emergency Stop (Kill Switch)**: Pressing `Esc` or `q` immediately aborts active agent tasks and locks the session.

---

## 5. Multi-Provider LLM Integration

* **Local / Privacy-First**: Direct HTTP client to local OpenAI-compatible daemons (Ollama at `http://127.0.0.1:11434`, `llama.cpp`, LocalAI). Zero wallet metadata leaves the machine.
* **Cloud Providers**: Google Gemini API, Anthropic, OpenAI. API keys are stored encrypted with AES-256-GCM in the user's Vaughan vault.

---

## 6. User Interface Integration

* **TUI Agent View** (`vaughan-tui/src/views/agent.rs`): Interactive chat interface with real-time streaming tokens, proposal approval cards, tool invocation logs, and quick action shortcuts.
* **CLI Agent** (`vaughan agent "<prompt>"`): Non-interactive execution for automated terminal workflows, scripts, and CI/CD pipelines.
