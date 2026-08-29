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

Details: [`README.md`](README.md) · Docs: [`docs/agent-configuration.md`](../../docs/agent-configuration.md)
