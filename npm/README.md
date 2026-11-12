# vaughan-mcp

A self-custody wallet for autonomous agents, as an MCP server.

The agent gets an **allowance**, not a key: keys stay inside a local Rust vault, and every
money path is charged against a persisted spend ledger (per-asset 24h cap + destination
allowlist). If the agent is confused, injected, or lying, the loss is bounded by the policy it
was given.

## Use it

Point any MCP client at the package. Nothing else to install — the first run downloads the
release binary for this platform and verifies its SHA256.

```json
{
  "mcpServers": {
    "vaughan": {
      "command": "npx",
      "args": ["-y", "vaughan-mcp"],
      "env": { "VAUGHAN_PROFILE": "default" }
    }
  }
}
```

`VAUGHAN_PROFILE=default` is **adviser** mode: the agent proposes, a human approves in the TUI.
`VAUGHAN_PROFILE=sentient` is the agent's **own wallet**: it executes without approval cards and is
bounded by `daily_cap_pct` / `allow_transfer_to` in the profile's `sentient-policy.toml`.

Either way the MCP process only proposes — run `vaughan serve --password-env <VAR>` (or keep the
TUI unlocked) on the same machine, because the vault stays where the human put it.

## Environment

| Variable | Meaning |
|---|---|
| `VAUGHAN_PROFILE` | Profile to expose (`default`, `sentient`). Ignored if `--profile` is passed. |
| `VAUGHAN_BIN` | Use an existing binary instead of downloading (e.g. a local build). |
| `VAUGHAN_VERSION` | Release tag to fetch. Defaults to this package's version. |
| `VAUGHAN_CACHE` | Where the binary is cached. Defaults to `~/.cache/vaughan-mcp`. |

## Why a wrapper

The binary is ~15 MB per platform and already published with checksums by the release workflow.
Downloading it once into a cache keeps `npx` installs instant and the version pinned, without
shipping three binaries inside an npm tarball.

Checksum verification is mandatory; `VAUGHAN_NO_VERIFY=1` exists only for a locally built mirror
and a mismatch always aborts.

## Support

linux-x64, linux-arm64, darwin-arm64. On anything else (Intel macOS, Windows), build from source
and set `VAUGHAN_BIN`:

```sh
cargo install --path vaughan-cli   # from a checkout
```

License: MIT.
