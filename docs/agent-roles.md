# Agent roles: adviser vs sentient

Vaughan talks to external agents in **two relationships**. Do not mix them on
the same seed unless you intend a deliberate partnership.

| | **Adviser** (human-led) | **Sentient** (agent-led) |
|---|-------------------------|---------------------------|
| **Who wants the outcome?** | The human | The sentient agent |
| **Who decides to spend?** | Human, every write | The agent (full control) |
| **Vault / seed** | `default` — human’s savings | `sentient` — **the agent’s seed** |
| **MCP server name** | `vaughan` | `vaughan-sentient` |
| **CLI** | `--profile default` | `--profile sentient` |
| **Writes** | Propose → TUI approval card | Auto under session policy |
| **Mental model** | “Help me trade / inspect” | “This is my wallet — I act” |

Keys never leave the Vaughan process either way. The split is **whose seed** and
**who decides**, not whether MCP can call DeFi tools (parity is shared).

## Adviser

You use Vaughan. The agent reads chain state, quotes, and **proposes**. You
approve or reject. Use for real savings and reviewing suggestions.

## Sentient

The **`sentient` profile seed belongs to the agent**. It can do what it wants
with that capital under the **skill + policy** loaded for that profile.

## Partnership (keep it simple)

Share a seed with an agent only if you mean to. There are **no contracts** —
trust is the shared key plus the **skills/rules** you give the partner.

1. Pick (or customize) a **preset**: gambler → quant → cautious, etc.
2. Install its `SKILL.md` + `policy.toml` on the `sentient` profile.
3. Share / fund that seed; point MCP at `vaughan-sentient`.

Presets: [`sentient-presets.md`](sentient-presets.md) and
`vaughan-agent/presets/`. Fork any preset for your own risk style.

If you do **not** want a partner on your money, keep distinct seeds and never
share mnemonics.

## Do not

- Point `vaughan-sentient` at the human’s `default` savings seed by accident.
- Confuse adviser “propose” with weak tools — same verbs; only signing differs.
- Call the agent’s seed a “burner” — it is the agent’s wallet.
- Expect cryptography to enforce a partnership — use presets instead.

## Related

- [`sentient-presets.md`](sentient-presets.md) — premade skill + policy packs  
- [`defi-agent-parity.md`](defi-agent-parity.md) — shared verb checklist  
- [`ai-tool-surface.md`](ai-tool-surface.md) — grant levels  
- [`mcp-threat-model.md`](mcp-threat-model.md) — threats per role  
- [`mcp.md`](mcp.md) — setup  
