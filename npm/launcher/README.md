# vaughan

Terminal wallet CLI/TUI and MCP server for PulseChain and EVM chains.

> **Security notice:** If you hold real funds, **do not use npm as your primary install path**.
> Install from a checksum-verified [GitHub Release tarball](https://github.com/r4-ndm/Vaughan-CLI/releases)
> (`SHA256SUMS`) or `scripts/install.sh`. The `vaughan` npm package is a **convenience, not recommended for a funded wallet**.

## Recommended Install (Checksum-Verified Release)

```bash
curl -fsSL https://raw.githubusercontent.com/r4-ndm/Vaughan-CLI/main/scripts/install.sh | sh
```

Or manually download `vaughan-<platform>.tar.gz` and `SHA256SUMS` from
https://github.com/r4-ndm/Vaughan-CLI/releases and verify with `sha256sum -c`.

## Convenience Install (npm)

```bash
npm install -g vaughan
vaughan --version
vaughan-mcp --version
```

How it works:

- `vaughan` ships **no postinstall script** and performs **no runtime download**.
- Your platform binary is installed via pinned `optionalDependencies`
  (`vaughan-linux-x64`, `vaughan-linux-arm64`, or `vaughan-darwin-arm64`) with npm integrity verification.
- Provides two executables:
  - `vaughan` — passes all arguments straight through to the native binary (TUI/CLI).
  - `vaughan-mcp` — runs `vaughan mcp ...` over stdio (or `--version` / `-V` to print the version).

## Supported Platforms

- `linux-x64` (`vaughan-linux-x64`)
- `linux-arm64` (`vaughan-linux-arm64`)
- `darwin-arm64` (`vaughan-darwin-arm64`)

On unsupported platforms (such as Windows or Intel macOS), `vaughan` exits with `UNSUPPORTED_PLATFORM` and prints the GitHub Releases URL so you can build from source:

```bash
cargo install --path vaughan-cli   # from a checkout
```

License: MIT.
