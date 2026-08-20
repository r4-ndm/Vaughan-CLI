---
name: pulsechain-context
description: PulseChain-oriented defaults and known tooling context for Vaughan.
mode: all
kind: guide
---

# PulseChain context

- Primary networks: PulseChain mainnet (369) and testnet v4 (943). Prefer testnet for risky experiments.
- Vaughan’s contract browser and sensory tools are alloy-native; no Foundry `cast` subprocess.
- DEX browsing is capability-based (V2 reserves / V3 slot0), not a hard-coded single DEX adapter.
- Stealth payments (ERC-5564) and EIP-7702 batching are separate wallet features — only discuss them if relevant; do not invent announcer or impl addresses from memory when tools can verify.
