# Vaughan on npm (`vaughan` & `vaughan-mcp`)

> **Security notice:** GitHub Releases + `scripts/install.sh` is the canonical, checksum-verified install for anyone holding real funds.
> Installing via npm is a **convenience, not recommended for a funded wallet**.

## Recommended Install (Checksum-Verified Release)

```sh
curl -fsSL https://raw.githubusercontent.com/r4-ndm/Vaughan-CLI/main/scripts/install.sh | sh
```

Or download the tarball and `SHA256SUMS` from [GitHub Releases](https://github.com/r4-ndm/Vaughan-CLI/releases) and verify with `sha256sum -c`.

## Human CLI/TUI + MCP via npm (`vaughan` — convenience)

`vaughan` installs the native binary for your platform via npm `optionalDependencies` (`vaughan-linux-x64`, `vaughan-linux-arm64`, `vaughan-darwin-arm64`) with **no postinstall script and no runtime download**, and exposes both `vaughan` and `vaughan-mcp`:

```sh
npm install -g vaughan
vaughan --version
vaughan-mcp --version
```

## Agent MCP wrapper (`vaughan-mcp`)

`vaughan-mcp` remains available as a self-custody MCP stdio server wrapper that downloads the pinned release binary on first run, verifies `SHA256SUMS`, and execs `vaughan mcp <args...>`:

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

### Environment (`vaughan-mcp` / `vaughan`)

| Variable | Meaning |
|---|---|
| `VAUGHAN_PROFILE` | Profile for `vaughan-mcp` (`default`, `sentient`). Ignored if `--profile` is passed. |
| `VAUGHAN_BIN` | Use an existing binary instead of a platform package or download. |
| `VAUGHAN_VERSION` | (`vaughan-mcp` wrapper) Release tag to fetch. Defaults to this package's version. |
| `VAUGHAN_CACHE` | (`vaughan-mcp` wrapper) Where the binary is cached. Defaults to `~/.cache/vaughan-mcp`. |

## Support

`linux-x64`, `linux-arm64`, `darwin-arm64`. On anything else (Intel macOS, Windows), the launcher prints `UNSUPPORTED_PLATFORM` with the Releases URL — build from source and set `VAUGHAN_BIN`:

```sh
cargo install --path vaughan-cli   # from a checkout
```

License: MIT.
