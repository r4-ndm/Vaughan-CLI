---
name: pulsechain-context
description: PulseChain defaults, known PulseX addresses, and connected-wallet rules.
mode: all
kind: guide
---

# PulseChain context

- Primary networks: PulseChain mainnet (369, native **PLS**) and testnet v4 (943, native **tPLS**). Prefer testnet for risky experiments.
- Always use the **SESSION CONTEXT** connected wallet for balances and swaps — never ask the user to paste their address if it is already listed there.
- Vaughan’s contract browser and sensory tools are alloy-native; no Foundry `cast` subprocess.
- Stealth payments (ERC-5564) and EIP-7702 batching are separate wallet features — only discuss them if relevant.

## Known DEX addresses (verify with tools when unsure)

### Mainnet (369)

| Role | Address |
|---|---|
| WPLS | `0xA1077a294dDE1B09bB078844df40758a5D0f9a27` |
| HEX | `0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39` |
| PLSX | `0x95B303987A60C71504D99Aa1b13B4DA07b0790ab` |
| PulseX V1 Router | `0x98bf93ebf5c380C0e6Ae8e192A7e2AE08edAcc02` |
| PulseX V2 Router | `0x165C3410fC91EF562C50559f7d2289fEbed552d9` |
| PulseX V2 Factory | `0x29eA7545DEf87022BAdc76323F373EA1e707C523` |
| PulseX V3 SwapRouter | `0xDA9aBA4eACF54E0273f56dfFee6B8F1e20B23Bba` |
| 9mm V2 Router | `0xcC73b59F8D7b7c532703bDfea2808a28a488cF47` |
| 9mm V2 Factory | `0x3a0Fa7884dD93f3cd234bBE2A0958Ef04b05E13b` |
| 9mm V3 SwapRouter | `0x7bE8fbe502191bBBCb38b02f2d4fA0D628301bEA` |
| 9mm V3 Factory | `0xe50DbDC88E87a2C92984d794bcF3D1d76f619C68` |
| 9inch V2 Router | `0xeB45a3c4aedd0F47F345fB4c8A1802BB5740d725` |
| 9inch V3 Router | `0x42556A17EF0Bd815bF21aD628DFd2e2f3b5F9ac7` |
| SparkSwap (dexSWAP) Router | `0x76C08825b4A675FD6a17A244660BabeB4ADA79d5` |
| Dextop / zkzx Router | `0x1f849694Ef24a2245bCa415FE47500216B24d7FF` |
| pDex V3 Router | `0x1eC2eaA62117486c9b2a05F098a7bF2568e19204` |
| Uniswap V3 SwapRouter (Hedron) | `0xE592427A0AEce92De3Edee1F18E0157C05861564` |
| PHUX Vault (Balancer) | `0x7F51AC3df6A034273FB09BB29e383FCF655e473c` |
| 0xTide entry | `0x634F6B9Cd1f860314871548d2224362825384B2D` |

DEX TUI (`d`): **↑/↓** venue · **←/→** V2/V3. Swappable today = Uni V2/V3 forks with catalogued routers (PulseX, 9mm, 9inch, SparkSwap, Dextop, Uniswap/Hedron, pDex). **Listed but not swap-wired:** PHUX / 0xTide (Balancer), 0xBistro / AgoraX / CURV (OTC), FiDex (no published router). Prefer **mainnet (369)** for most venues; testnet mainly PulseX V2.

### Testnet v4 (943)

| Role | Address |
|---|---|
| WPLS (tWPLS) | `0x70499adEBB11Efd915E3b69E700c331778628707` |
| **Prefer first:** PulseX (early) Factory | `0xFf0538782D122d3112F75dc7121F61562261c0f7` |
| Matching early Router | `0xDaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7` |
| PulseX V2 Factory | `0x29eA7545DEf87022BAdc76323F373EA1e707C523` (often empty / unused on v4) |
| PulseX V2 Router | `0x636f6407B90661b73b1C0F7e24F4C79f624d0738` |

On testnet, call `search_pairs` on the **early** factory first — do not treat WPLS as a factory.
For native tPLS spends, leave a little headroom for gas (Vaughan will reject if value+gas exceeds balance and tell you the max `amount_in`).
Match router to the factory that owns the pair.
For tHEX / other testnet tokens not listed here, ask for the token address or `inspect_contract` a candidate; do not invent token addresses.

## Piteas aggregator

- Quote client lives in `vaughan-core::core::piteas` (see `docs/piteas.md`).
- Public beta: `GET https://sdk.piteas.io/quote` — native PLS spelled `PLS`; router `0x6BF228eb7F8ad948d37deD07E595EfddfaAF88A6`.
- Partner API key (when issued): encrypt into `piteas.key.json`; set `auth_style` in `piteas.toml`. Do not put keys in chat or skills.

## Aggregator TUI (`g`)

- **Primary (no key):** SquirrelSwap Brain — `https://api.squirrelswap.pro` (`POST /swap` → unsigned tx). Native PLS = `0x000…000`.
- Also live: PulseSwap, Piteas public beta.
- Listed: Switch.win (needs key), Empseal, 9mm 9X, CURV — see `docs/aggregator.md`.
- Bridge (`f`): LibertySwap USDC cross-chain (`docs/bridge.md`) — not official Omnibridge;
  destination is async (no claim tracker in v1). Omnibridge/PulseRamp is deferred.
- Browser (`c`) intent macros: `/swap`, `/inspect 0x…`, `/revoke`, `/stealth receive`
  (thin jumps to Ag / browse / Approvals / Receive). Writes: `write` / `writeraw` → fee confirm.
