# wiz4rd-swap addresses (Vaughan copy)

Mirrored from the sibling **wiz4rd-swap** repo (`docs/addresses.md`, deployed
2026-08-20). Canonical Rust constants: `vaughan_core::core::wiz4rd`.

## PulseChain Testnet V4 (943)

| Contract | Address |
|----------|---------|
| PancakeV3PoolDeployer | `0x55DC1d6155363CE68BB525ce473126a3d192574E` |
| PancakeV3Factory | `0x297BeFB564d3Bba2D1913613B84Fb743C259C6cf` |
| SwapRouter | `0xfC656c95eCd418536844FeeaA46949bb9365BEaF` |
| NonfungiblePositionManager | `0xf1b1D004dD8bFC618F977F6ACAD127a60c566745` |
| QuoterV2 | `0x38d1752597c2c0BD25E980891cd6d74766138FB7` |
| TickLens | `0xEE88CDf0D030d733A1E2a1fD9E6Ab3780DE7B768` |
| WPLS | `0x70499adEBB11Efd915E3b69E700c331778628707` |
| WZRD (smoke ERC-20) | `0x29bab93456c0E97EE931C1554c7C215480aa7766` |
| Smoke pool WZRD/WPLS @ 500 | `0xd47E01C1Af55a48C11d0E324fb1853cf2501e0Dc` |

Fee tiers: `100`, `500`, `2500`, `10000`, `20000` (2%).

## Mainnet (369)

Not deployed yet.

## How to use in Vaughan

1. Unlock TUI → set network to **PulseChain testnet v4**.
2. Dex (`d`) → venue **Wiz4rd** (default on 943) · protocol **V3** · fee **500**.
3. Token in/out: WPLS and/or WZRD smoke token; approve + swap via existing Dex confirm.
4. MCP: `get_network` includes a `wiz4rd` object when `chain_id == 943`.

Browse (`c`) can inspect factory / router / NPM / smoke pool directly.

Agent LP/mint tools are still Phase D — see [`wiz4rd-agent-plan.md`](wiz4rd-agent-plan.md).
