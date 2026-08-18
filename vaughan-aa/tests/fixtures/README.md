# Ambire signed-transaction fixtures

Fixtures for `tests/differential.rs` are **byte vectors only** — no Ambire or
other third-party source is ever committed here. They are captured with
**Solidity's own `abi.encode`, executed by the EVM** (a throwaway forge
project), which is the canonical encoder the on-chain `AmbireAccount.execute`
verifies against — a stronger, dependency-free reference than Ambire's TS SDK
(which itself just delegates to `ethers`' encoder).

## Capture procedure

The throwaway forge project lives in `vaughan-aa/.fixtures-capture/`
(gitignored — only the resulting JSON here is committed):

```sh
cd vaughan-aa/.fixtures-capture
forge test --match-test test_generate -vv   # writes JSON to out/fixtures/
cp out/fixtures/*.json ../tests/fixtures/
```

`test/Generate.t.sol` defines a set of representative `scw_transaction` cases
and computes, per case, the reference values via `src/DigestHelper.sol`
(a ~30-line MIT helper; the Ambire interface facts it references are declared
inline, never copied from Ambire's AGPL source):

- `preimage`          = `abi.encode(account, chainId, nonce, txns)`
- `digest`            = `keccak256(preimage)`
- `execute_calldata`  = `abi.encodeCall(execute, (txns, signature))` with a
  fixed 66-byte `0xaa…` signature (pins the *encoding*; signature-content
  correctness is covered by `vaughan-aa`'s sign tests)

The Rust harness (`tests/differential.rs`) asserts all three byte-for-byte
against the same inputs. When no `*.json` files are present, it skips.

## Fixture schema

```json
{
  "name": "case1-single-zero-value",
  "account": "0x…",
  "chain_id": 943,
  "nonce": 0,
  "txns": [
    { "to": "0x…", "value": "0", "data": "0xdeadbeef" }
  ],
  "preimage": "0x…",
  "digest": "0x…",
  "signature": "0xaa…aa",
  "execute_calldata": "0x…"
}
```

`value` may be decimal or `0x`-prefixed; `data` is `0x`-prefixed. `preimage`,
`signature`, and `execute_calldata` are optional (older fixtures assert only
`digest`).

## Current cases

| Fixture | Shape |
|---|---|
| `case1-single-zero-value` | one txn, zero value, `0xdeadbeef` data |
| `case2-native-transfer` | one txn, 1 ETH value, empty data |
| `case3-multi-txn-batch` | 3 txns: native transfer + ERC-20 `transfer` + `approve` |
| `case4-empty-batch` | zero txns |
| `case5-erc20-transfer` | one txn, padded `transfer(address,uint256)` calldata |
