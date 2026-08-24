# Sentient / partner presets

Premade **skill + policy** packs for humans who share a seed (or fund
`sentient`) with an agent. No contracts — behavior comes from these files.

See [`docs/sentient-presets.md`](../../docs/sentient-presets.md).

| Preset | Path |
|--------|------|
| `high-risk-gambler` | [`high-risk-gambler/`](high-risk-gambler/) |
| `balanced` | [`balanced/`](balanced/) |
| `quant-risk-reward` | [`quant-risk-reward/`](quant-risk-reward/) |
| `cautious` | [`cautious/`](cautious/) |

Each folder: `PRESET.md` (human blurb) + `SKILL.md` (LLM rules) + `policy.toml`
(circuit-breaker dials → copy to `degen-policy.toml`).
