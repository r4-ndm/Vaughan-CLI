# Vaughan AI Agent Configuration & User Guide

> **Retired (2026-08-23):** Embedded in-wallet LLM chat, provider setup, and
> `vaughan agent` / `vaughan policy` CLI are removed. Agents now use
> [`docs/mcp.md`](mcp.md) (`vaughan mcp`). The `vaughan-agent` crate remains as
> a library (proposals, tools, circuit breakers). This document is historical
> reference only.
>
> **Update (2026-08-29):** operating mode is no longer picked on a welcome
> screen — it is keyed to the **profile you pick at unlock** (FR-5.1), and the
> sentient kill-switch is **Ctrl+K** (not Esc). See
> [`docs/mcp-threat-model.md`](mcp-threat-model.md) for the current controls.

This guide explains how to configure and use the AI Agent subsystem (`vaughan-agent`) across **AI-Assisted** and **Sentient** operating modes.

---

## 1. Quick Start: Choosing an AI Provider

Vaughan supports both **100% private local models** (zero API key) and **cloud LLM providers** (Gemini, OpenAI, OpenRouter, DeepSeek). Chat I/O is handled by the Rust [`genai`](https://crates.io/crates/genai) multi-provider client so new OpenAI-compatible endpoints plug in via `agent.toml` without custom HTTP code.

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

When you pick **AI Assisted** or **Sentient** on the Vaughan welcome screen, Vaughan
asks which provider to use:

1. **Ollama (local)** — no API key; optional model name (default `llama3.2`)
2. **Google Gemini** — paste API key → choose model:
   - **Gemini 3.5 Flash** (`gemini-3.5-flash`)
   - **Gemini 3.5 Pro** (`gemini-3.5-pro`)

   OpenAI’s `gpt-oss-120b` is **not** available with a Gemini API key — that model
   lives on Vertex AI MaaS (`gpt-oss-120b-maas`) only. Vaughan remaps any leftover
   `gpt-oss-*` ids in `agent.toml` to Flash.
3. **OpenAI / compatible** — paste API key → optional model (OpenRouter, DeepSeek, …)
4. **Cursor gateway** — paste Cursor API key (`crsr_…`) → **OpenAI-compatible chat
   gateway base URL** (required) → optional model (default `composer-2`)

   Cursor’s official `api.cursor.com` host is Cloud Agents / Admin / SDK only — it
   does **not** implement `POST /v1/chat/completions`, so Vaughan cannot talk to it
   directly for Assist chat. Point `endpoint_url` / `CURSOR_BASE_URL` at a gateway
   that speaks OpenAI chat completions (for example a local proxy on
   `http://127.0.0.1:8765`). For a simple cloud chat path, use Gemini or OpenAI
   instead.

The key is encrypted with your vault password (`agent.key.json`) and never written
in plaintext. Non-secret settings land in `agent.toml` beside the wallet.

Env overrides for Cursor: `CURSOR_API_KEY`, **required** `CURSOR_BASE_URL` (chat
gateway), optional `CURSOR_MODEL`.

Press **`s`** to skip and keep using environment variables / Ollama defaults.

If you unlock in Assist/Sentient and there is still no `agent.toml` (or a cloud
provider without a key), Vaughan opens the same AI setup screen before the
dashboard so you can finish configuration without restarting.

### C. Environment variables (automation / CI)

You can still provide your API key via standard environment variables:

#### Google Gemini
```bash
export GEMINI_API_KEY="AIzaSy..."
export GEMINI_MODEL="gemini-3.5-flash" # optional, defaults to gemini-3.5-flash
```

#### OpenAI / OpenRouter / DeepSeek / LocalAI
Vaughan supports any OpenAI-compatible endpoint:
```bash
# Official OpenAI
export OPENAI_API_KEY="sk-proj-..."
export OPENAI_MODEL="gpt-4o"

# OpenRouter (keys start with sk-or-v1-…)
export OPENAI_API_KEY="sk-or-v1-..."
export OPENAI_BASE_URL="https://openrouter.ai/api"   # optional — auto-detected from sk-or- keys
export OPENAI_MODEL="openrouter/free"
```

If you paste an OpenRouter key during AI setup, Vaughan stores `endpoint_url` as
`https://openrouter.ai/api` automatically so the key is not sent to api.openai.com.

```bash
# DeepSeek
export OPENAI_API_KEY="sk-..."
export OPENAI_BASE_URL="https://api.deepseek.com"
export OPENAI_MODEL="deepseek-chat"
```

---

## 2. Profile-Specific Configuration (`agent.toml`)

To persist model configurations across sessions without setting environment variables each time, create an `agent.toml` file inside your profile directory (e.g. `~/.vaughan/profiles/default/agent.toml` or `~/.vaughan/profiles/sentient/agent.toml`):

```toml
# Provider options: "ollama", "gemini", "openai"
provider = "gemini"

# Model identifier
model = "gemini-3.5-flash"

# Environment variable name holding the API key (keeps keys out of plaintext config)
api_key_env = "GEMINI_API_KEY"

# Optional: custom endpoint URL for local or third-party gateways
# endpoint_url = "https://generativelanguage.googleapis.com"

# Sampling temperature (0.0 = deterministic, 0.7 = creative)
temperature = 0.2
```

---

## 4. Agent skills (rules + guides)

Vaughan injects markdown **skills** into the LLM system prompt for Assist / Sentient:

- **Index (any agent):** [`vaughan-agent/skills/INDEX.md`](../vaughan-agent/skills/INDEX.md)
- Bundled: `vaughan-agent/skills/*/SKILL.md` (compiled into the binary)
- User overrides: `<profile>/skills/*/SKILL.md` (same `name` replaces bundled)

Notable guides for MCP hosts:

| Skill | When |
|-------|------|
| [`vb-ag-quotes`](../vaughan-agent/skills/vb-ag-quotes/SKILL.md) | Ag quote tours, `browser_open_agg`, Switch.win VB path |
| [`dapp-connect`](../vaughan-agent/skills/dapp-connect/SKILL.md) | dApp inject / connect bugs |
| [`pulsechain-context`](../vaughan-agent/skills/pulsechain-context/SKILL.md) | PLS/WPLS/HEX addresses |

`kind: must` skills are mandatory safety/signing rules. `kind: guide` skills are
workflow tips (contract inspection, PulseChain context). See
`vaughan-agent/skills/README.md`.

---

## 5. How to Use the AI Agent

### Interactive TUI Chat REPL
Inside `vaughan-tui`, press **`g`** on the dashboard to open the AI Agent screen.

Free-form questions stream token-by-token from the configured LLM (Ollama by default;
OpenAI/Gemini via env keys). **Esc** cancels an in-flight turn.

### Switch models in chat (OpenCode-style)

Type **`/model`** (or **`/models`**) in the prompt bar to open a picker for the
current provider — ↑/↓ to move, type to filter, Enter to select, Esc to cancel.
You can also set a model directly:

```
/model gemini-3.5-pro
/model ollama/llama3.2
```

Same-provider switches apply immediately and update `agent.toml`. Switching to a
different provider (needs a new API key / endpoint) uses **`/provider`**.

### Tooling commands

Slash-style commands still work for deterministic tooling:

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
| **Sentient Mode** | ⚡ Autonomous Execution | 🔑 Burner Key (Isolated Profile) | 🛡️ Rust Circuit Breakers & Multi-RPC Quorum |

1. **No Key Exposure**: The `vaughan-agent` crate has zero imports of the vault decryption module. Even if an LLM is prompt-injected, it cannot extract keys.
2. **Ground-Truth UI**: All confirmation dialogs independently decode bytecode directly from the Ethereum RPC without relying on LLM-generated explanations.
3. **Secret Zeroization**: All API keys and vault secrets use `secrecy::SecretString` with automatic memory zeroization upon drop.
4. **Propose-after-sense**: Assist mode refuses `propose_*` tools unless a sensory tool already succeeded in the same turn.
5. **Sentient dry-run**: set `VAUGHAN_SENTIENT_DRY_RUN=1` (or call `SentientTrader::with_dry_run(true)`) to run circuit breakers + simulation without broadcasting.
