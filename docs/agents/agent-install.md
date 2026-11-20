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
sweeps and CLI sends — is charged against the spend ledger (`sentient-spend.json`) before it
signs, under a lock, and the file survives restarts. In the recommended setup the ledger lives in
the signer daemon's store (`/var/lib/vaughan/sentient`, owned by `vaughan-signer`); a TUI-hosted
profile keeps it under `profiles/<name>/`. A failed broadcast still costs budget; a retried action
is refused rather than re-run.

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

## 4. Start the signer (sentient) or the vault (adviser)

The MCP process is a proposer. For **sentient**, the keys belong to `vaughan signerd` — a
root-installed systemd service running as user `vaughan-signer`, reachable only through
`/run/vaughan/signerd.sock` — so there is nothing to unlock and the agent's process never holds a
password:

```sh
sudo bash scripts/install-signerd.sh   # one-off: store, ownership, vaughan-signerd.service
vaughan --profile sentient serve        # optional thin client: no vault unlock, no password,
                                        # no mcp.session token, no TCP port
```

`vaughan mcp --profile sentient` can reach the socket directly, so that thin client is convenience,
not a requirement.

For **adviser** (`default`) the vault itself must be unlocked on the same machine, because every
proposal waits for a human approval card:

```sh
export VAUGHAN_WALLET_PASSWORD='…'      # secret manager / short-lived injection only
vaughan serve --password-env VAUGHAN_WALLET_PASSWORD
# or interactively: vaughan --profile default
```

## 5. Give the agent a budget

Percentage caps are sized from the account's **live on-chain balance**, read fresh on every request
(a failed balance read refuses the intent rather than falling back to a default). Stage them
through the signer rather than editing the store by hand:

```sh
vaughan --profile sentient policy set --max-position-pct 10 --daily-cap-pct 20
vaughan policy pending            # show the staged change
sudo vaughan signerd approve      # as root, on a clean console: prints the diff, asks y/N
```

The live file is `<store>/sentient-policy.toml` (`/var/lib/vaughan/sentient`, owned by
`vaughan-signer`); a staged change sits in `policy-pending.json` (10-minute TTL) until root
approval writes the one-time `policy-approval.json`, and applied diffs are appended to
`policy-audit.jsonl`. **No vault password is involved** — that path was deleted in v0.2.10.

Fund the sentient profile with what you are willing to lose — that balance, not the policy, is the
hardest limit in the system. Ask the agent to call `budget_status` to see what it has left.

## Verifying instead of trusting

Every claim above is checkable: the ledger is a plain JSON file in the signer store (0600, so read
it as root), the allowlist and caps are in the policy file, policy changes are logged, and
`cargo test -p vaughan-agent spend_ledger` exercises the cap, the restart survival, the lock and
the duplicate-key refusal. A wallet an agent is asked to trust should be one its operator can
test.
