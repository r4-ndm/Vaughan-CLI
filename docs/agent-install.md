# Installing Vaughan for an agent

Two audiences install this: a human who wants their agent to hold a wallet, and a runtime that
is assembling an agent and needs a tool. Both paths end in the same place — an MCP stdio server
on the same machine as the vault.

## What an agent gets, and what it cannot do

| The agent can | The agent cannot |
|---|---|
| read balances, quotes and token metadata without approval | touch the human's `default` (adviser) wallet |
| propose transfers, swaps, LP steps and DCA plans | exceed the per-asset 24h cap in its own profile |
| execute autonomously **in its own `sentient` profile** | send to an address absent from `allow_transfer_to` |
| ask how much budget remains (`budget_status`) | read, export or unlock the vault |

The bound is not advice in a config file: every money path — proposals, DCA slices, stealth
sweeps and CLI sends — is charged against `profiles/<name>/sentient-spend.json` before it signs,
under a lock, and the file survives restarts. A failed broadcast still costs budget; a retried
action is refused rather than re-run.

## 1. Install the binary

```sh
curl -fsSL https://raw.githubusercontent.com/r4-ndm/Vaughan-CLI/main/scripts/install.sh | sh
```

Installs to `~/.local/bin/vaughan` after verifying the release SHA256. If `~/.local/bin` is not on
`PATH` ahead of other copies, check `command -v vaughan` — two installed binaries with different
dates is the usual cause of "the feature is in the repo but not in my binary".

## 2. Or let the client fetch it: `npx`

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

The package downloads the pinned release for your platform once, verifies the checksum, caches it
in `~/.cache/vaughan-mcp`, then execs `vaughan mcp`. `VAUGHAN_BIN=/path/to/vaughan` overrides it.

## 3. Register with a client

```sh
vaughan mcp install --client cursor          # ~/.cursor/mcp.json
vaughan mcp install --client claude          # `claude mcp add-json`, or ~/.claude.json
vaughan mcp install --client codex           # ~/.codex/config.toml
vaughan mcp install --client claude-desktop  # ~/.config/Claude/claude_desktop_config.json
vaughan mcp install --client all --dry-run   # show what would change, write nothing
```

It backs up any file it modifies, never rewrites a file it cannot parse, and is safe to re-run.

## 4. Start the vault

The MCP process is a proposer. Something has to hold the unlocked vault on the same machine:

```sh
# headless (agents, containers, cron)
VAUGHAN_PW=... vaughan serve --profile sentient --password-env VAUGHAN_PW

# or interactively, with the TUI open and unlocked
vaughan --profile default
```

Adviser mode needs the TUI for approvals; sentient mode runs headless under policy.

## 5. Give the agent a budget

In `profiles/sentient/sentient-policy.toml`:

```toml
enforcement = "enforced"
max_position_pct = 10    # per trade, % of that asset's balance
daily_cap_pct = 20       # cumulative, per asset, rolling 24h
allow_transfer_to = []   # empty = all plain transfers refused; list addresses to permit
```

Fund the sentient profile with what you are willing to lose — that balance, not the policy, is the
hardest limit in the system. Ask the agent to call `budget_status` to see what it has left.

## Verifying instead of trusting

Every claim above is checkable: the ledger is a JSON file you can read, the allowlist is in the
policy file, and `cargo test -p vaughan-agent spend_ledger` exercises the cap, the restart
survival, the lock and the duplicate-key refusal. A wallet an agent is asked to trust should be
one its operator can test.
