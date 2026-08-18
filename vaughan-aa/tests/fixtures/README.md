# Ambire signed-transaction fixtures

Fixtures for `tests/differential.rs` are captured **outside** this workspace,
because Ambire's SDK is GPL-family and must never be checked into (or absorbed
by) this MIT/Apache repo. Only the resulting **byte vectors** are committed here.

## Capture procedure (run in a separate checkout, not in this repo)

1. In a throwaway directory, `git clone` `AmbireTech/ambire-common` and install
   its deps (Node — never run inside Vaughan-CLI itself, which is Rust-only).
2. For a set of representative `scw_transaction`s (single txn, multi-txn batch,
   non-zero `value`, non-empty `data`), sign them with the SDK and record the
   SDK's own computed digest.
3. Write each case to a JSON file here using the schema below. **Do not copy any
   `.ts`/`.sol` source** — only the recorded byte vectors.

## Fixture schema

```json
{
  "account": "0x1111111111111111111111111111111111111111",
  "chain_id": 943,
  "nonce": 0,
  "txns": [
    { "to": "0x2222222222222222222222222222222222222222", "value": "0", "data": "0xdeadbeef" }
  ],
  "digest": "0x…"
}
```

`digest` is the SDK's `keccak256(abi.encode(account, chainId, nonce, txns))`.
`value` may be decimal or `0x`-prefixed. `data` is `0x`-prefixed.

When no `*.json` files are present, `tests/differential.rs` skips.
