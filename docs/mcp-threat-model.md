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

- User approves what is shown on the approval card.
- OS-level keyloggers, clipboard sniffers, shoulder surfing.
- Smart contract exploits after a correct approval (user accepted calldata).

## Controls matrix

| Threat | Control |
|--------|---------|
| Agent lies in `explanation` | Labeled **untrusted**; calldata decode is authoritative |
| Stale simulation | Re-simulate `eth_call` at approve; block if revert |
| Wrong chain | `chain_id` on proposal; reject approve on mismatch |
| Queue file tampering | HMAC-SHA256 over proposal bytes + session secret |
| Local socket hijack | Loopback only (`127.0.0.1:8746`); random session token in `mcp.session` (0600) |
| MCP exfiltrates keys | MCP process never unlocks vault; banned tools + tests |
| Mainnet accident | Testnet default; `VAUGHAN_MCP_ALLOW_MAINNET=1` for mainnet writes |
| Approval flooding | Max 10 pending proposals; rate limit per profile |
| Fee spike | Re-estimate at approve; warn if >10% delta vs proposal |
| TOCTOU | Re-simulate + re-estimate fee at approve time |
| Double-spend proposal | Terminal states; idempotent by `proposal_id` |

## Trust boundaries

```
┌─────────────────────────────────────────┐
│  UNTRUSTED: Cursor / agent / MCP process │
│  - May call read + propose tools only    │
│  - Never holds vault password or keys    │
└──────────────────┬──────────────────────┘
                   │ loopback socket / file queue
                   ▼
┌─────────────────────────────────────────┐
│  TRUSTED: Vaughan TUI (unlocked)         │
│  - Owns WalletState + signing            │
│  - Unified approval with EIP-1193        │
└─────────────────────────────────────────┘
```

## v2 note

A future `vaughan serve` daemon moves the trusted boundary to a long-running
process. v1 IPC types (`ProposalQueue`, session token, HMAC) become the daemon
wire protocol — no throwaway work.
