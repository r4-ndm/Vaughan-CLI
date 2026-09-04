# HEX stakes (pHEX on PulseChain)

On-chain HEX stake **reads** and **writes** for PulseChain mainnet (369).

Patterns inspired by [pulsechain-mcp](https://github.com/DavidFeder/pulsechain-mcp)
`hexStake` helpers; reimplemented in Rust under `vaughan-core::core::hex_stake`
(no TypeScript vendoring).

## Contracts

| Label | Address | Staking |
|-------|---------|---------|
| **pHEX** | `0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39` | Yes (state-fork) |
| **eHEX** | `0x57fde0a71132198BBeC939B98976993d8D89D225` | No (bridged ERC-20) |

Hearts use **8 decimals**. Not a price oracle.

## MCP tools

| Tool | Role |
|------|------|
| `hex_global_state` | `currentDay` + `globals` (soft-fail for eHEX) |
| `hex_stakes_for_address` | `stakeCount` + `stakeLists` |
| `propose_hex_stake_start` | Draft `stakeStart(hearts, days)` on **pHEX only** — never signs |
| `propose_hex_stake_end` | Draft `stakeEnd(index, stakeId)` on **pHEX only** — never signs |

Writes refuse eHEX and arbitrary custom `0x` addresses. Sensory reads may still
use `contract=ehex` (soft-fail) or custom for research.

## TUI

Footer chip **`u` HEX** opens the stake manager (PulseChain 369):

- List stakes + day / shareRate summary
- **`s`** start stake (amount + days → confirm)
- **Enter** end selected stake (warns on early end penalty)
- **`r`** reload

## Proposal review

MCP approval cards now decode known selectors (transfer / approve / wrap /
HEX stakeStart|stakeEnd / swaps) into a verify table plus **Safety:** hints
(unlimited approve, early stake end, eHEX-as-stake target, etc.). Agent
`explanation` remains UNTRUSTED.
