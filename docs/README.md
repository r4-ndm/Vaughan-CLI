# docs/

Reference for people and agents. Start at the repo root's `AGENTS.md` /
`CLAUDE.md` for the rules; this tree is supporting detail, organized so a
subfolder name tells you what kind of claim the doc makes.

| Folder | What's in it |
|---|---|
| [`architecture/`](architecture/) | How a shipped feature or integration actually works: aggregator, bridge, browser engine, Ambire AA, ERC-5564 stealth, hardware wallets, MCP, the DEX/Ag venues. Read these to understand code that exists today. |
| [`agents/`](agents/) | Agent-facing surface: how `vaughan-agent` roles/presets/skills are configured and installed. |
| [`decisions/`](decisions/) | Go/no-go records for things we deliberately did *not* build as originally scoped (Kohaku, RAILGUN pin policy) and why. |
| [`risk/`](risk/) | Threat models and risk registers: dApp connection risk, MCP threat model, the security delta table. |
| [`roadmap/`](roadmap/) | Forward-looking plans and strategy — not yet shipped, or shipped partially. Treat as proposals, not a description of current behavior. |
| [`ops/`](ops/) | Operator runbooks and manual smoke checklists (kill switches, MCP smoke tests, demo recording scripts). |

Load-bearing docs stay at this top level because other files point at them
directly and they must never go stale from a rename:

- [`audit-playbook.md`](audit-playbook.md) — the audit procedure (`AGENTS.md`,
  `CLAUDE.md`, and `.claude/skills/audit/SKILL.md` all point here)
- [`audit-findings.md`](audit-findings.md) — dated findings log from past audits
- [`fable-5-audit-prompt.md`](fable-5-audit-prompt.md) — prompt used to run an
  audit with a different model as a second opinion

A few self-contained directories aren't docs in this sense and aren't
categorized above: [`examples/`](examples/) (sample fixtures),
[`Native-Equivalent/`](Native-Equivalent/), [`railgun-upstream/`](railgun-upstream/)
(vendored-interop patches, see its own `README.md`), and [`spikes/`](spikes/).
