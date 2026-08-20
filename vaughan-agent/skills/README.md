# Vaughan Agent Skills

Markdown playbooks injected into the AI system prompt for Assist / Degen modes.

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
mode: all | assist | degen
kind: must | guide
---
```

- **`kind: must`** — mandatory rules (cannot be overridden by user chat).
- **`kind: guide`** — reference workflow tips.
- **`mode`** — which operating mode loads the skill (`all` = both AI modes).

## Bundled skills

| Skill | Kind | Mode |
|-------|------|------|
| `core-rules` | must | all |
| `assist-advisor` | must | assist |
| `degen-trader` | must | degen |
| `contract-inspection` | guide | all |
| `pulsechain-context` | guide | all |

## User overrides

Place the same folder layout under the profile directory:

`<data_dir>/vaughan-cli/skills/...` (default profile)  
or `<data_dir>/vaughan-cli/profiles/<name>/skills/...`

A user skill with the same `name` **replaces** the bundled one.
