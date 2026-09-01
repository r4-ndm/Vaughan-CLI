# Agent skills index

**Any external agent** (Cursor, Claude Code, Codex, Vaughan Assist/Sentient) should
start here to find bundled playbooks.

## Canonical location

```
vaughan-agent/skills/<skill-name>/SKILL.md
```

User overrides (same `name` replaces bundled):

```
~/.local/share/vaughan-cli/skills/<skill-name>/SKILL.md
~/.local/share/vaughan-cli/profiles/<profile>/skills/<skill-name>/SKILL.md
```

Cursor also mirrors project skills at `.cursor/skills/<skill-name>/SKILL.md` — those
files **point here**; edit `vaughan-agent/skills/` first.

## When to read which skill

| User intent | Read first |
|-------------|------------|
| Connect MCP, restart/reconnect host, tools missing, `wallet_locked` | [`mcp-connect/SKILL.md`](mcp-connect/SKILL.md) |
| Ag quotes, PLS→HEX tour, Switch.win UI, `browser_open_agg` | [`vb-ag-quotes/SKILL.md`](vb-ag-quotes/SKILL.md) |
| dApp connect / inject / CSP / trusted URL | [`dapp-connect/SKILL.md`](dapp-connect/SKILL.md) |
| PulseChain addresses, WPLS/HEX, routers | [`pulsechain-context/SKILL.md`](pulsechain-context/SKILL.md) |
| Contract fingerprint / ABI inspect | [`contract-inspection/SKILL.md`](contract-inspection/SKILL.md) |
| Signing safety (always) | [`core-rules/SKILL.md`](core-rules/SKILL.md) |
| Confirm F1 network + F3 wallet before propose_* (switch via TUI F1/F3) | [`wallet-account/SKILL.md`](wallet-account/SKILL.md) |
| Ticker → paste `0x` → confirm contract before propose | [`token-resolve/SKILL.md`](token-resolve/SKILL.md) |
| Deposit vs wallet balance — warn before propose (LP legs) | [`balance-preflight/SKILL.md`](balance-preflight/SKILL.md) |
| createPool gas / reverted deploy / pool missing after Y | [`lp-gas-preflight/SKILL.md`](lp-gas-preflight/SKILL.md) |
| LP Brew bugs (ghost card, mint stuck, MCP timeout, swapped amounts) | [`lp-brew-incidents/SKILL.md`](lp-brew-incidents/SKILL.md) |
| V3 LP deploy Brews (token-agnostic, 943) — casual “create pool on wiz4rd” → Q&A script | [`vaughan-brews/SKILL.md`](vaughan-brews/SKILL.md) · [`conversational-brew.md`](vaughan-brews/conversational-brew.md) |
| Stuck Brew, missing approval card, failed Y, new session resume | [`workflow-recovery/SKILL.md`](workflow-recovery/SKILL.md) |

## All bundled skills

| Skill | Kind | Path |
|-------|------|------|
| `core-rules` | must | [`core-rules/SKILL.md`](core-rules/SKILL.md) |
| `assist-advisor` | must | [`assist-advisor/SKILL.md`](assist-advisor/SKILL.md) |
| `sentient-trader` | must | [`sentient-trader/SKILL.md`](sentient-trader/SKILL.md) |
| `mcp-connect` | guide | [`mcp-connect/SKILL.md`](mcp-connect/SKILL.md) |
| `vb-ag-quotes` | guide | [`vb-ag-quotes/SKILL.md`](vb-ag-quotes/SKILL.md) |
| `dapp-connect` | guide | [`dapp-connect/SKILL.md`](dapp-connect/SKILL.md) |
| `pulsechain-context` | guide | [`pulsechain-context/SKILL.md`](pulsechain-context/SKILL.md) |
| `contract-inspection` | guide | [`contract-inspection/SKILL.md`](contract-inspection/SKILL.md) |
| `token-resolve` | guide | [`token-resolve/SKILL.md`](token-resolve/SKILL.md) |
| `balance-preflight` | guide | [`balance-preflight/SKILL.md`](balance-preflight/SKILL.md) |
| `lp-gas-preflight` | guide | [`lp-gas-preflight/SKILL.md`](lp-gas-preflight/SKILL.md) |
| `lp-brew-incidents` | guide | [`lp-brew-incidents/SKILL.md`](lp-brew-incidents/SKILL.md) |
| `wallet-account` | guide | [`wallet-account/SKILL.md`](wallet-account/SKILL.md) |
| `workflow-recovery` | guide | [`workflow-recovery/SKILL.md`](workflow-recovery/SKILL.md) |
| `vaughan-brews` | guide | [`vaughan-brews/SKILL.md`](vaughan-brews/SKILL.md) |

Details: [`README.md`](README.md) · Docs: [`docs/agent-configuration.md`](../../docs/agent-configuration.md)
