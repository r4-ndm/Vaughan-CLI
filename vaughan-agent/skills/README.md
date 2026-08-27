# Vaughan Agent Skills

Markdown playbooks injected into the AI system prompt for Assist / Sentient modes.

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
| `pulsechain-context` | guide | all |
| `dapp-connect` | guide | all |

### `dapp-connect`

Per-URL connect playbooks under [`dapp-connect/sites/`](dapp-connect/sites/)
(SquirrelSwap, LibertySwap, PulseX IPFS directory, 9inch CSP, …). Read the site
file before changing inject/provider code for a connect bug.

## Sentient partner presets

Premade **skill + `policy.toml`** packs (gambler → quant → cautious) for humans
who share a seed with an agent — no contracts, just rules. See
[`../presets/`](../presets/) and [`docs/sentient-presets.md`](../../docs/sentient-presets.md).

## User overrides

Place the same folder layout under the profile directory:

`<data_dir>/vaughan-cli/skills/...` (default profile)  
or `<data_dir>/vaughan-cli/profiles/<name>/skills/...`

A user skill with the same `name` **replaces** the bundled one.
