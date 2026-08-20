# Vaughan AI Agent Configuration & User Guide

This guide explains how to configure and use the AI Agent subsystem (`vaughan-agent`) across **AI-Assisted** and **Degen** operating modes.

---

## 1. Quick Start: Choosing an AI Provider

Vaughan supports both **100% private local models** (zero API key) and **cloud LLM providers** (Gemini, OpenAI, OpenRouter, DeepSeek).

### A. Local Models with Ollama (Default & Zero-API-Key)
If you run Ollama locally on your machine, Vaughan connects out-of-the-box with **zero configuration and zero API keys**:

1. Install and run Ollama:
   ```bash
   ollama run llama3.2
   ```
2. Start Vaughan in AI-Assisted mode:
   ```bash
   vaughan --mode assist
   ```
   All contract inspection and reasoning happen 100% locally. Zero telemetry, zero network leakage.

---

### B. Welcome-screen setup (recommended)

When you pick **AI Assisted** or **Degen** on the Vaughan welcome screen, Vaughan
asks which provider to use:

1. **Ollama (local)** — no API key; optional model name (default `llama3.2`)
2. **Google Gemini** — paste API key (masked) → optional model
3. **OpenAI / compatible** — paste API key → optional model (OpenRouter, DeepSeek, …)

The key is encrypted with your vault password (`agent.key.json`) and never written
in plaintext. Non-secret settings land in `agent.toml` beside the wallet.

Press **`s`** to skip and keep using environment variables / Ollama defaults.

If you unlock in Assist/Degen and there is still no `agent.toml` (or a cloud
provider without a key), Vaughan opens the same AI setup screen before the
dashboard so you can finish configuration without restarting.

### C. Environment variables (automation / CI)

You can still provide your API key via standard environment variables:

#### Google Gemini
```bash
export GEMINI_API_KEY="AIzaSy..."
export GEMINI_MODEL="gemini-1.5-pro" # optional, defaults to gemini-1.5-flash
```

#### OpenAI / OpenRouter / DeepSeek / LocalAI
Vaughan supports any OpenAI-compatible endpoint:
```bash
# Official OpenAI
export OPENAI_API_KEY="sk-proj-..."
export OPENAI_MODEL="gpt-4o"

# OpenRouter
export OPENAI_API_KEY="sk-or-v1-..."
export OPENAI_BASE_URL="https://openrouter.ai/api/v1"
export OPENAI_MODEL="anthropic/claude-3.5-sonnet"

# DeepSeek
export OPENAI_API_KEY="sk-..."
export OPENAI_BASE_URL="https://api.deepseek.com/v1"
export OPENAI_MODEL="deepseek-chat"
```

---

## 2. Profile-Specific Configuration (`agent.toml`)

To persist model configurations across sessions without setting environment variables each time, create an `agent.toml` file inside your profile directory (e.g. `~/.vaughan/profiles/default/agent.toml` or `~/.vaughan/profiles/degen/agent.toml`):

```toml
# Provider options: "ollama", "gemini", "openai"
provider = "gemini"

# Model identifier
model = "gemini-1.5-pro"

# Environment variable name holding the API key (keeps keys out of plaintext config)
api_key_env = "GEMINI_API_KEY"

# Optional: custom endpoint URL for local or third-party gateways
# endpoint_url = "https://generativelanguage.googleapis.com"

# Sampling temperature (0.0 = deterministic, 0.7 = creative)
temperature = 0.2
```

---

## 4. Agent skills (rules + guides)

Vaughan injects markdown **skills** into the LLM system prompt for Assist / Degen:

- Bundled: `vaughan-agent/skills/*/SKILL.md` (compiled into the binary)
- User overrides: `<profile>/skills/*/SKILL.md` (same `name` replaces bundled)

`kind: must` skills are mandatory safety/signing rules. `kind: guide` skills are
workflow tips (contract inspection, PulseChain context). See
`vaughan-agent/skills/README.md`.

---

## 5. How to Use the AI Agent

### Interactive TUI Chat REPL
Inside `vaughan-tui`, press **`g`** on the dashboard to open the AI Agent screen.

Free-form questions stream token-by-token from the configured LLM (Ollama by default;
OpenAI/Gemini via env keys). **Esc** cancels an in-flight turn. Slash-style commands
still work for deterministic tooling:

* **Inspect a Smart Contract**:
  ```
  inspect 0x165C3410fC91EF562C50559f7d2289fEbed552d9
  ```
  *The agent inspects on-chain bytecode, detects the ERC-20 / Uniswap / WETH standard interface, and resolves ABI selectors.*

* **Check Balances**:
  ```
  balance 0x70997970C51812dc3A010C7d01b50e0d17dc79C8
  ```

* **Draft a Transaction Proposal**:
  ```
  transfer 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 1000000000000000000
  ```
  *The agent simulates the call via `eth_call` pre-flight check, builds a typed `TxProposal` card showing the exact raw calldata, value, and fee, and waits for you to press `[a] Approve` or `[d] Deny`.*

CLI free-form prompts stream the same way:
```bash
vaughan --mode assist agent "What does this PulseX router do?"
```

---

### Non-Interactive CLI Commands
You can invoke the agent directly from your terminal or shell scripts:

```bash
# Contract inspection via sensory tools
vaughan agent "inspect 0x2222222222222222222222222222222222222222" --mode assist

# Query balance
vaughan agent "balance" --mode assist

# Draft a transfer proposal
vaughan agent "transfer 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 1000000000000000000" --mode assist
```

---

## 6. Security Boundaries & Protection Guarantees

| Operating Mode | AI Capability | Private Key Access | Confirmation Gate |
|---|---|---|---|
| **Human Only** | ❌ Disabled (zero code loaded) | 🔒 Locked in Vault | 100% Manual |
| **AI Assisted** | 🧠 Advisor (Sensory + Proposals) | ❌ Structurally impossible | 🔒 Mandatory Human Confirmation (Decoded Calldata) |
| **Degen Trader** | ⚡ Autonomous Execution | 🔑 Burner Key (Isolated Profile) | 🛡️ Rust Circuit Breakers & Multi-RPC Quorum |

1. **No Key Exposure**: The `vaughan-agent` crate has zero imports of the vault decryption module. Even if an LLM is prompt-injected, it cannot extract keys.
2. **Ground-Truth UI**: All confirmation dialogs independently decode bytecode directly from the Ethereum RPC without relying on LLM-generated explanations.
3. **Secret Zeroization**: All API keys and vault secrets use `secrecy::SecretString` with automatic memory zeroization upon drop.
4. **Propose-after-sense**: Assist mode refuses `propose_*` tools unless a sensory tool already succeeded in the same turn.
5. **Degen dry-run**: set `VAUGHAN_DEGEN_DRY_RUN=1` (or call `DegenTrader::with_dry_run(true)`) to run circuit breakers + simulation without broadcasting.
