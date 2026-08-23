# Bridge screen (LibertySwap)

`f` opens **LibertySwap** cross-chain — a convenience wrapper around their public
quote API (quote → approve if needed → broadcast on the **source** chain).

This is **not** the official Pulse Omnibridge (`bridge.pulsechain.com`). Label
in the TUI says so on purpose.

## Live API

| | |
|---|---|
| Base | `https://apis.libertyswap.finance/v3` |
| Quote | `GET /swap/quote?srcToken&dstToken&amount&srcChain&dstChain&recipient` |
| Auth | **None** |
| Rate limit | ~30 req/min |

Docs still mention `api.libertyswap.finance/v1/quote`; the web app uses **`/v3/swap/quote`**
and **requires `recipient`**. Vaughan follows the live app.

## Vaughan flow

1. Set **From** / **To** (Pulse ↔ Base / Eth / BSC / Arb / Polygon).
2. Active wallet **Net must match From**.
3. Amount in **USDC** human units (min ≈ 10).
4. Enter → quote → confirm approve (if any) → confirm bridge → source tx hash.
5. Destination arrival is async — check the dest chain later (no tracker in v1).

Routers in the quote response are checked against an allowlist (docs + known
unified router). Unknown `to` → refuse.

## Tests

```bash
cargo test -p vaughan-core --lib bridge::
cargo test -p vaughan-core live_quote_usdc_pulse_to_base -- --ignored --nocapture
```

## Related

- Official Omnibridge = later, separate track
- Ag (`g`) = same-chain swap aggregators only
- `docs/aggregator.md` — LibertyX points here
