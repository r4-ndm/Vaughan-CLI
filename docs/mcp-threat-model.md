# Vaughan MCP Threat Model (v1)

Threat model for external agents using Vaughan via MCP stdio and hybrid IPC.
Bind this document before shipping MCP v1.

## In-scope adversaries

1. **Compromised or malicious external agent** — prompt injection, wrong
   arguments, lying in `explanation` text.
2. **Other local processes** — port scan on loopback, tamper with queue files.
3. **Malicious or stale RPC** — wrong chain state, failed simulations hidden
   from the agent.

## Out of scope (explicit)

- User approves what is shown on the **adviser** (`default`) approval card.
- Outcomes on a **sentient** seed the agent (or partner) chose to fund.
- Shared-seed partnership without a skill preset (user’s choice; not Vaughan’s fault).
- OS-level keyloggers, clipboard sniffers, shoulder surfing.
- Smart contract exploits after a correct approval / policy-allowed auto-exec.

## Product model (adviser vs sentient)

Full write-up: [`agent-roles.md`](agent-roles.md).

| Role | Trust assumption | Control |
|------|------------------|---------|
| **Adviser** (`default` / `vaughan`) | Agent may be wrong; human wants help | Every write needs human approve |
| **Sentient** (`sentient` / `vaughan-sentient`) | Agent acts on **its** seed (or shared partnership seed + skill preset) | Auto-exec under policy; don’t conflate with human `default` |

Primary separation is **whose seed** and **who decides**. Partnership = shared
seed + [`sentient-presets.md`](sentient-presets.md) — no on-chain contracts.

## Controls matrix

| Threat | Control |
|--------|---------|
| Agent lies in `explanation` | Labeled **untrusted**; calldata decode is authoritative |
| Stale simulation | Re-simulate `eth_call` at approve (default) or pre-broadcast (sentient) |
| Wrong chain | `chain_id` on proposal; reject on mismatch |
| Queue file tampering | HMAC-SHA256 over proposal bytes + session secret |
| Local socket hijack | Loopback only (`127.0.0.1:8746`); random session token in `mcp.session` (0600) |
| MCP exfiltrates keys | MCP process never unlocks vault; banned tools + tests |
| Agent spends human savings | Use distinct seeds; only share mnemonic for intentional partnership |
| Mainnet accident | Testnet default; `VAUGHAN_MCP_ALLOW_MAINNET=1` for mainnet writes |
| Approval flooding | Max 10 pending proposals; rate limit per profile |
| Runaway sentient agent | Profile policy + circuit breakers + Esc kill-switch |
| Fee spike | Re-estimate at approve / pre-broadcast; warn if >10% delta |
| TOCTOU | Re-simulate + re-estimate fee before sign |
| Double-spend proposal | Terminal states; idempotent by `proposal_id` |

## Trust boundaries

```
┌─────────────────────────────────────────┐
│  UNTRUSTED: Cursor / agent / MCP process │
│  - Read + write tools (propose or auto)  │
│  - Never holds vault password or keys    │
└──────────────────┬──────────────────────┘
                   │ loopback socket / file queue
                   ▼
┌─────────────────────────────────────────┐
│  TRUSTED: Vaughan (unlocked profile)     │
│  - Owns WalletState + signing            │
│  - adviser (default): human approval     │
│  - sentient: policy auto-sign            │
└─────────────────────────────────────────┘
```

## v2 note

A `vaughan serve` daemon moves the trusted boundary to a long-running
unlocked process (password via `--password-env`). MCP stdio remains unprivileged.
v1 IPC types (`ProposalQueue`, session token, HMAC) become the daemon wire
protocol — no throwaway work.
