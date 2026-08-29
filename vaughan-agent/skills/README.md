# Vaughan Agent Skills

Markdown playbooks for **any agent** (Cursor, Claude Code, Codex, Vaughan Assist/Sentient).

**Find a skill:** [`INDEX.md`](INDEX.md) — intent → path lookup table.

## Layout

```
skills/
  <skill-name>/
    SKILL.md          # required
```

Each `SKILL.md` starts with frontmatter:

```yaml
---
name: short-id
description: one-line summary
mode: all | assist | sentient
kind: must | guide
---
```

- **`kind: must`** — mandatory rules (cannot be overridden by user chat).
- **`kind: guide`** — reference workflow tips.
- **`mode`** — which operating mode loads the skill (`all` = both AI modes). Sentient sessions use `sentient`. Skill frontmatter is consumed by external MCP hosts (Cursor, Claude, etc.) — not parsed by Vaughan Rust; update host configs that still filter on legacy `degen`.

## Bundled skills

| Skill | Kind | Mode |
|-------|------|------|
| `core-rules` | must | all |
| `assist-advisor` | must | assist |
| `sentient-trader` | must | sentient |
| `contract-inspection` | guide | all |
| `mcp-connect` | guide | all |
| `pulsechain-context` | guide | all |
| `dapp-connect` | guide | all |
| `vb-ag-quotes` | guide | all |

### `mcp-connect`

Wire Vaughan into **Cursor**, **Claude Desktop**, or **Claude Code**; reconnect MCP after
config changes. Per-host UI steps under [`mcp-connect/hosts/`](mcp-connect/hosts/).
Read before `vb-ag-quotes` when tools are missing or `wallet_locked`.

### `dapp-connect`

Per-URL connect playbooks under [`dapp-connect/sites/`](dapp-connect/sites/)
(SquirrelSwap, LibertySwap, PulseX IPFS directory, 9inch CSP, …). Read the site
file before changing inject/provider code for a connect bug.

### `vb-ag-quotes`

Ag catalog **quote tours** via Vaughan Browser + MCP (`browser_open_agg`,
snapshot/click/type). Per-venue steps under [`vb-ag-quotes/venues/`](vb-ag-quotes/venues/)
(Switch.win without API key, PulseSwap deep links, EmpX/PortalX browserless-only).
Read before running multi-venue PLS→HEX comparisons in VB.

## Sentient partner presets

Premade **skill + `policy.toml`** packs (gambler → quant → cautious) for humans
who share a seed with an agent — no contracts, just rules. See
[`../presets/`](../presets/) and [`docs/sentient-presets.md`](../../docs/sentient-presets.md).

## User overrides

Place the same folder layout under the profile directory:

`<data_dir>/vaughan-cli/skills/...` (default profile)  
or `<data_dir>/vaughan-cli/profiles/<name>/skills/...`

A user skill with the same `name` **replaces** the bundled one.
