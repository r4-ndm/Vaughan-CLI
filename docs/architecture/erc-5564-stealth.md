# ERC-5564 stealth — spec freeze (Vaughan)

> Status: **GO for scheme-1 crypto in `vaughan-core`**. TUI / announcer deploy /
> scan-sweep are not in this slice.
> Recorded 2026-08-19.

## Product claim (v1)

A stealth payment is **unlinkable to the recipient’s published public address**.

It is **not** a private transaction: sender, amount, and token remain on-chain.
Sweeping a stealth note from the main account links it. v1: the **sender**
attaches a small PLS stipend so the recipient can move funds without that link.

## Cryptography (scheme id `1`)

Pinned to [ERC-5564](https://eips.ethereum.org/EIPS/eip-5564) scheme 1 and
ScopeLift [`stealth-address-sdk`](https://github.com/ScopeLift/stealth-address-sdk)
(`@noble/secp256k1` `getSharedSecret`, default compressed point):

1. Shared point `S = ephemeral_sk · viewing_pk` (also `viewing_sk · ephemeral_pk`).
2. `h(S) = keccak256(SEC1 compressed S)` — **33 bytes** (`02`/`03` ‖ x), not the
   32-byte x-only ECDH secret.
3. View tag = `h(S)[0]`.
4. Stealth pubkey = `spending_pk + h(S)·G` (scalar `h(S)` reduced mod n).
5. Stealth address = Ethereum address of that pubkey.
6. Stealth private key = `spending_sk + h(S) (mod n)`.

Do **not** use Kohaku or `eth-stealth-addresses`.

## HD paths (frozen)

Derived from the vault mnemonic (empty BIP-39 passphrase), hardened:

| Role | Path |
|------|------|
| Spend | `m/5564'/60'/0'/0'` |
| View  | `m/5564'/60'/0'/1'` |

Changing these makes every published meta-address unrestorable. BIP-44
`m/44'/60'/0'/0/i` is unchanged (public EOA).

Abandon-mnemonic lock in `vaughan-core` tests:

- spend pubkey `027346ef4cc9362fe4c90ba060cc341eab788046139db0626e1b17908aed6c6441`
- view pubkey `02fc10565657ef01035e3197e43fdcbdc4017c8556cb8d43bcae7f68aa79f0d1b4`

## URI

`st:<shortName>:0x<33-byte spend compressed hex><33-byte view compressed hex>`

Example short names: `pls` (PulseChain 369), `tpls` (testnet 943), `eth`.
v1 copy-pastes this URI. ERC-6538 registry is optional later.

## Canonical contracts

| Role | Address |
|------|---------|
| Announcer | `0x55649E01B5Df198D18D95b5cc5051630cfD45564` |
| Registry (later) | `0x6538E6bf4B0eBd30A8Ea093027Ac2422ce5d6538` |
| CREATE2 factory | `0x4e59b44847b379578588920cA78FbF26c0B4956C` |

Factory **is** on PulseChain 369 and testnet 943. Compiled runtime matches
Ethereum mainnet bytecode. CREATE2 with the EIP salt lands at the canonical
address.

- **943 announcer: live** (2026-08-19), codesize 709, tx
  `0x1df79490a33e146b4915a0cab2e293f2b711c07f08a966b3a3795d6ad070ce98`
  (block 25174175).
- **943 E2E:** send → announce → scan → sweep passed (`tests/stealth_943.rs`).
  Example sweep: `0x14b694f5ac0bdaeb227b1dd65fe2b616e29d836075eac9e85879edd480fb68fd`.
- **Anvil:** core + TUI coverage includes Alice→Bob (scan isolation), two notes,
  dust-stipend sweep refusal, scan after later blocks, TUI sweep, and missing
  announcer / invalid `st:` URI.
- **369 announcer:** still empty. Same script with a mainnet RPC after 943 E2E.

```bash
PRIVATE_KEY=0x... ./scripts/deploy-erc5564-announcer.sh
# default RPC is https://rpc.v4.testnet.pulsechain.com
```

## Out of this freeze

- Mainnet 369 announcer (after using the TUI on 943)
- ERC-20 stealth metadata
- ERC-6538 registration
- RAILGUN / Kohaku
