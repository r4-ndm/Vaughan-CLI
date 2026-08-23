# Security Policy

Vaughan-CLI is self-custody wallet software. Treat every release as high-risk
until you have reviewed it yourself. Never enter a real mnemonic or mainnet
password into a build you do not trust.

## Supported versions

| Version | Supported |
| ------- | --------- |
| latest release tag | yes |
| `main` branch | best-effort (pre-release) |
| older tags | no |

## Reporting a vulnerability

**Do not open a public GitHub issue for security bugs.**

DM **[@VaughanWallet](https://x.com/VaughanWallet)** on X with:

1. A clear description of the issue and impact
2. Steps to reproduce (prefer a minimal test case on testnet / Anvil)
3. Affected version or commit SHA
4. Your suggested fix, if you have one

We aim to acknowledge within 72 hours and share a remediation timeline when
possible.

## Scope

In scope:

- Vault encryption, key handling, memory zeroization, and secret logging
- Transaction signing, approval flows, and broadcast paths
- EIP-1193 provider bridge and dApp approval UX
- AI agent tools that can propose or execute on-chain actions
- Dependency supply-chain issues in this repository's direct build

Out of scope:

- Third-party RPC endpoints, LLM providers, or dApp contracts
- User machine compromise (malware, shoulder surfing, clipboard sniffers)
- Loss of funds from user-approved transactions that executed as shown

## Safe disclosure

We follow coordinated disclosure. Please give us reasonable time to patch
before public disclosure. We will credit reporters who wish to be named.
