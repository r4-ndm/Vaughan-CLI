# LP Brew — session check (wallet + chain)

Full rules: [`wallet-account/SKILL.md`](../wallet-account/SKILL.md).

## When

Immediately after `get_control_plane_status` + `get_network` succeed — **before** token
or pool questions.

## One question

> You are on **{network name} ({chain_id})**, **{account_label}** (`0x…short…`).
> Is this the **chain and wallet** you want to deploy from?

- **Yes** → continue (tokens, pool detection, …).
- **No** → user switches in TUI; agent waits and re-checks.

## Switch instructions

| Change | TUI |
|--------|-----|
| Network | **F1** → **↑/↓** → **Enter** |
| Wallet | **F3** → **↑/↓** → **Enter** |

wiz4rd testnet LP → expect **943**. If wrong chain, switch **F1** first.

## Never

Call `propose_v3_lp_deploy` until user confirms session or you re-check after a switch.
