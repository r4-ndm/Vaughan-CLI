# Ag venue index (VB quotes)

Quick lookup for **`browser_open_agg`** and per-site steps. Full workflow:
[`../SKILL.md`](../SKILL.md).

## Same-chain PLS → HEX (use these)

| MCP `venue` | Label | VB / API | Playbook |
|-------------|-------|----------|----------|
| `squirrel` | SquirrelSwap | API + VB | [`squirrelswap.md`](squirrelswap.md) |
| `pulseswap` | PulseSwap | API + VB (deep link) | [`pulseswap.md`](pulseswap.md) |
| `piteas` | Piteas | API + VB | [`piteas.md`](piteas.md) |
| `switch` | Switch.win | VB only (API needs key) | [`switch-win.md`](switch-win.md) |
| `9mm` | 9mm 9X | VB (`9x.9mm.pro`) | [`nine-mm-9x.md`](nine-mm-9x.md) |
| `curv` | CURV / Jolt | VB (Switch.win UI) | [`curv.md`](curv.md) |
| `empx` | EmpX | **API only** (`quote_swap`) | [`empx.md`](empx.md) |

## Not same-chain PLS → HEX — skip this tour

| MCP `venue` | Why |
|-------------|-----|
| `libertyx` | **Bridge** — USDC cross-chain (`libertyswap.finance`). Use Bridge screen (`f`), not Ag PLS→HEX. |
| `internetmoney` | Wallet marketing site — no wired swap UI. |
| `portalx` | Cross-chain portal — not wired. |

**Open example:** `{ "venue": "switch", "pls_hex": true }` → MCP tool `browser_open_agg`.

URLs: `vaughan-core/src/core/aggregator/catalog.rs` (`web_url_pls_hex` = same-chain only).
