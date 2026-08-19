# AI Agent Architecture & Security Sandboxing Specification

**Version**: 1.1 (Hardened Post-Audit)  
**Status**: Approved Specification  
**Applies to**: `vaughan-agent`, `vaughan-core`, `vaughan-tui`, `vaughan-cli`

---

## 1. Overview & Core Philosophy

Vaughan AI Agent integration extends the sovereign wallet foundation (`vaughan-core` + `wiz4rd-engine`) with autonomous and semi-autonomous AI intelligence without ever compromising private key security, user privacy, or sovereign self-custody.

The architecture strictly enforces three fundamental axioms:
1. **The AI Agent is an Advisor by default, never a Signer without physical capital isolation.**
2. **Ground-Truth UI Rendering is non-negotiable:** The confirmation modal displays independently decoded on-chain bytecode, ignoring all LLM-generated persuasive text.
3. **Physical Isolation for Autonomy:** Autonomous execution (Degen Mode) runs in a dedicated burner sub-profile with separate keys; primary funds are physically unlinked and inaccessible.

---

## 2. The 3-Tier Operating Mode Hierarchy

Operating mode is chosen **once at startup / onboarding** and is **permanently immutable for the lifetime of that process session**.

```
                              STARTUP / WELCOME
                                      │
         ┌────────────────────────────┼────────────────────────────┐
         ▼                            ▼                            ▼
  [1] Pure Human Mode         [2] AI Assist Mode          [3] Degen Bot Mode
  • Zero AI code initialized  • AI Advisor loaded         • Autonomous Agent
  • Zero LLM network calls    • Propose-Only (Read safe)  • Isolated burner wallet
  • 100% manual wallet        • Human MUST approve tx     • Dual-horizon gas caps
  • CANNOT switch to AI       • CANNOT auto-execute       • Hard slippage ceilings
```

### 1. Mode 1: Pure Human Mode (`OperatingMode::HumanOnly`)
* **Structural Enforcement**: The `AgentEngine` struct is `Option::None` in memory. Zero background workers are spawned, zero LLM memory is allocated, and the Agent view and shortcuts are completely omitted from the UI router.
* **Compile-Time Feature Flag**: Vaughan provides an optional `cargo build --no-default-features` path that completely compiles out the `vaughan-agent` crate for pure sovereign cold storage.
* **Session Lock**: Impossible to switch to AI assist during the session.

### 2. Mode 2: AI Assist Mode (`OperatingMode::AiAssisted`)
* **Purpose**: Active DeFi users seeking contract inspection, arbitrage scanning, transaction simulation, and natural language batch composition.
* **Security Model**: The agent has **zero access to private keys or signing capabilities**.
* **Execution**: Autonomous for read queries (balances, contracts, DEX reserves); **strictly requires manual human confirmation (Enter/PIN)** for any transaction proposal.

### 3. Mode 3: Degen Bot Mode (`OperatingMode::DegenTrader`)
* **Storage Isolation**: Runs in a **dedicated isolated profile directory** (`~/.vaughan/profiles/degen/`) with its own independent seed phrase and funds.
* **Safety**: Primary savings/vault are physically unlinked and inaccessible on the filesystem.
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
└──────────────────────────────┘
```

### Layer 1: Architectural Air-Gap & Memory Isolation
* When the vault is unlocked with the master password, it unseals into two strictly isolated structs:
  1. `SignerContext` (holds derived private keys — passed **only** to the manual approval gate or degen worker).
  2. `AgentConfig` (holds API keys / endpoint configuration — passed to `vaughan-agent`).
* `vaughan-agent` receives **zero memory references** to `SignerContext` or `Vault`. It only takes a `ReadOnlyProvider` and `ProposalSink`.

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

### Layer 3: Anti-Prompt Injection & Ground-Truth UI Rendering
* **Untrusted Input Isolation**: On-chain data (contract names, token symbols, event logs, revert messages) are treated as untrusted user input and enclosed in structured JSON schemas.
* **Ground-Truth UI Separation**: The TUI confirmation modal strictly separates verified cryptographic data from LLM commentary:
  - **[Verified On-Chain Data]**: Checksummed recipient address, on-chain verified token symbol/decimals, exact native balance delta, and decoded function parameters.
  - **[AI Commentary (Untrusted)]**: Rendered in a separate visual box with an explicit warning banner indicating that text is unverified model output.

### Layer 4: Prompt Context Scoping & Egress Prevention
* The agent prompt builder strictly limits injected context to the minimum required parameters for the immediate query (e.g. target address, queried token balance).
* Historical transaction logs, full contact books, and unassociated wallet accounts are **never injected** into the model prompt context.

---

## 4. Degen Mode Circuit Breakers (PulseChain-Calibrated)

In `DegenTrader` mode, autonomous signing is governed by deterministic limits enforced in Rust:

1. **Position Sizing Limit**: No single transaction may exceed `X%` of the active degen wallet balance (default: 20%).
2. **Dual-Horizon Gas Ceilings**:
   - **Rolling Window**: Max `X PLS` gas spent within 10 minutes.
   - **Cumulative Session Budget**: Hard lifetime gas cap (e.g. `Y PLS`). Once reached, the agent enters a mandatory hard stop requiring human re-authorization.
3. **Gas-to-Value Ratio Tripwire**: Automatically rejects any transaction where estimated gas cost exceeds 5% of the transaction's economic value.
4. **Adaptive Slippage Ceiling**: Price impact is calculated against pool reserves using `wiz4rd-engine`. Trades exceeding calculated depth or a 1.0%–2.5% safety wall are blocked from execution.
5. **Emergency Stop (Kill Switch)**: Pressing `Esc` or `q` immediately aborts active agent tasks, cancels pending operations, and locks the session.

---

## 5. Multi-Provider LLM Integration

* **Local / Privacy-First**: Direct HTTP client to local OpenAI-compatible daemons (Ollama at `http://127.0.0.1:11434`, `llama.cpp`, LocalAI). Zero wallet metadata leaves the machine.
* **Cloud Providers**: Google Gemini API, Anthropic, OpenAI. API keys are stored encrypted with AES-256-GCM in the user's Vaughan vault and decrypted only into `AgentConfig`.

---

## 6. User Interface Integration

* **TUI Agent View** (`vaughan-tui/src/views/agent.rs`): Interactive chat interface with real-time streaming tokens, proposal approval cards, tool invocation logs, and quick action shortcuts.
* **Welcome Screen 3-Way Selector**: Initial onboarding and startup view allowing explicit selection between `[1] Pure Human`, `[2] AI Assisted`, and `[3] Degen Bot`.
* **CLI Agent** (`vaughan agent "<prompt>"`): Non-interactive execution for automated terminal workflows, scripts, and CI/CD pipelines.
