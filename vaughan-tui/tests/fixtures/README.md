# Anvil fixtures for browserless Pulse paths

| File | Role |
|------|------|
| `mock_weth.runtime.hex` | Minimal WETH9-shaped ERC-20 (`deposit` / `withdraw` / `approve` / `allowance` / `balanceOf`). Plant with `anvil_setCode`. |

Regenerate with Foundry: compile a matching `MockWeth.sol`, then write
`deployedBytecode.object` into this file (with or without a `0x` prefix).
