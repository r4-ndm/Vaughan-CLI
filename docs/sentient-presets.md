# Sentient skill presets

When a human **shares a seed** with a sentient agent (or funds the agent's
`sentient` profile), they do **not** need on-chain contracts. They pick a
**preset** — bundled skill rules + matching `sentient-policy.toml` dials — so the
partner agent behaves the way they want.

Presets are starting points. Copy into the profile and edit freely.

## How to use

1. Choose a preset below (or fork one).
2. Copy `SKILL.md` → profile skills dir (replaces/extends sentient behavior):
   - `~/.vaughan/…/profiles/sentient/skills/<name>/SKILL.md`
3. Copy `policy.toml` → profile as `sentient-policy.toml` (breaker numbers):
   - `~/.vaughan/…/profiles/sentient/sentient-policy.toml`

Or one shot:

```bash
vaughan --profile sentient preset apply balanced
vaughan preset list
```

4. Point MCP at `vaughan-sentient` / `--profile sentient` (TUI must be unlocked on
   that profile — proposals auto-exec after re-sim + policy).
5. Customize: edit the skill text and `/policy set …` anytime.

Bundled sources live under `vaughan-agent/presets/`.

## Premade presets

| Id | Vibe | Policy skew | Skill emphasis |
|----|------|-------------|----------------|
| `high-risk-gambler` | YOLO / meme / size up | High position %, looser slippage | Chase momentum; accept loss; no "wait for perfect" |
| `balanced` | Default partner | Mid caps | Sense → size → trade; don't revenge-trade |
| `quant-risk-reward` | Math-first | Tight slippage; smaller max % | Explicit R:R, size from edge; skip bad expectancy |
| `cautious` | Capital preservation | Small %; tight slippage; enforced | Prefer sim / skip; rarely max size |

No contracts, pacts, or dual-sig. Trust = shared seed + these rules. Change the
preset when the partnership's risk appetite changes.

## Custom presets

Same layout:

```
my-preset/
  SKILL.md       # frontmatter + behavioral rules for the LLM
  policy.toml    # AgentSessionPolicy fields (see sentient policy)
  PRESET.md      # optional one-liner for humans
```

`SKILL.md` frontmatter for sentient partner skills:

```yaml
---
name: my-preset
description: …
mode: sentient
kind: must
---
```

## Related

- [`agent-roles.md`](agent-roles.md) — adviser vs sentient  
- [`../vaughan-agent/skills/README.md`](../vaughan-agent/skills/README.md) — skill loading  
- [`../vaughan-agent/presets/`](../vaughan-agent/presets/) — files to copy  
