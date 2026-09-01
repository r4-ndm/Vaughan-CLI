# Brew JSON files (user-owned — not bundled presets)

The repo ships **no ticker-specific Brew presets**. Every user picks their own tokens.

## Where to put your Brews

```
~/.local/share/vaughan-cli/brews/my-pool.json
~/.local/share/vaughan-cli/profiles/<profile>/brews/my-pool.json
```

Copy [`brew.example.json`](brew.example.json) and fill in **your** token addresses.

## CLI

```bash
vaughan lp plan --brew ~/.local/share/vaughan-cli/brews/my-pool.json
vaughan lp deploy --brew /path/to/my-pool.json
```

Requires vault unlock; `deploy` also needs Advisor TUI unlocked (MCP session).

## Smoke / dev only

For CI and agent smoke tours on 943, see [`docs/examples/lp-brew-smoke-943.example.json`](../../docs/examples/lp-brew-smoke-943.example.json) — not a product preset.
