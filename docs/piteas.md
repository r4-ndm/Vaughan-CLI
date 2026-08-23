# Piteas aggregator integration

Vaughan talks to [Piteas](https://piteas.io/) (PulseChain DEX aggregator /
Pathfinder) for best-route quotes. Execution still goes through the user’s
wallet: approve (if ERC-20 in) → send `methodParameters` to **PiteasRouter** →
explicit TUI approval. No auto-sign.

## Status

| Piece | State |
|---|---|
| Quote HTTP client (`vaughan-core::core::piteas`) | Done |
| Encrypted partner API key (`piteas.key.json`) | Done (optional — for higher limits once issued) |
| `piteas.toml` (base URL + auth style) | Done |
| TUI Ag “Piteas” venue | Done — `LiveNoKey` via public `sdk.piteas.io` (no key required) |
| Agent `quote_piteas` tool | Not yet |

Public SDK beta works **without** a key today (`https://sdk.piteas.io/quote`,
~10 req/min). Partner keys are for higher limits / dedicated access once Piteas
issues them — not a blocker for Ag.

## Contracts (mainnet 369)

| Role | Address |
|---|---|
| PiteasRouter | `0x6BF228eb7F8ad948d37deD07E595EfddfaAF88A6` |
| PTE token | `0x2A06a971fE6ffa002fd242d437E3db2b5cC5B433` |

Native PLS in/out uses the string `PLS` in quote params (not the WPLS address).

## Quote API (beta)

```
GET https://sdk.piteas.io/quote
  ?tokenInAddress=PLS|0x…
  &tokenOutAddress=PLS|0x…
  &amount=<base units>
  &allowedSlippage=0.50
  &account=<optional receiver>
```

Response includes `methodParameters.calldata` + `methodParameters.value`. Send
that tx **to PiteasRouter** after user approval.

Official docs: <https://docs.piteas.io/piteas-sdk-api>

## Partner API key (when issued)

1. Ask Piteas which auth style they use (`bearer` / `x-api-key` / `query`).
2. Write settings next to the wallet profile:

```toml
# <data_dir>/vaughan-cli/piteas.toml
base_url = "https://sdk.piteas.io"   # or partner host they give you
auth_style = "bearer"                # none | bearer | x-api-key | query
max_requests_per_minute = 30
```

3. Store the key encrypted with the vault password (same Argon2id + AES-256-GCM
   as agent LLM keys):

```rust
use secrecy::SecretString;
use vaughan_core::core::piteas::{save_api_key, save_file_config, AuthStyle, PiteasFileConfig};

save_file_config(dir, &PiteasFileConfig {
    base_url: "https://sdk.piteas.io".into(),
    auth_style: AuthStyle::Bearer, // flip when they confirm
    max_requests_per_minute: 30,
})?;
save_api_key(dir, &vault_password, &SecretString::from(key))?;
```

Never commit keys, never log them, never put them in `piteas.toml`.

## Partner request template (send to Piteas)

Use when asking for production / elevated access (Telegram / X per their docs):

- **Product:** Vaughan-CLI — Rust PulseChain-first wallet TUI (local signing, no custody).
- **Use case:** In-wallet best-route swaps via Pathfinder; quotes only, user always approves.
- **Traffic (estimate):** start ~1–5 quotes/user session; hard client cap at configured RPM (default 10 for beta).
- **Users:** desktop/local installs (not a shared hosted portal).
- **Auth preference:** whatever they issue — we already support Bearer / X-API-Key / query.

## Rust usage sketch

```rust
use alloy::primitives::U256;
use vaughan_core::core::piteas::{
    load_api_key, load_file_config, NativeToken, PiteasClient, PiteasFileConfig, QuoteRequest,
};

let cfg = load_file_config(dir)?.unwrap_or_default();
let key = password.and_then(|pw| load_api_key(dir, pw).ok().flatten());
let client = PiteasClient::from_config(&cfg, key)?;

let quote = client
    .quote(&QuoteRequest::new(
        NativeToken::Pls,
        NativeToken::Address(dai),
        U256::from(10u64).pow(U256::from(18u64)),
    ))
    .await?;

let to = quote.method_parameters.router_address()?;
let data = quote.method_parameters.calldata_bytes()?;
let value = quote.method_parameters.value_u256()?;
// → show Approve / Send UI, then sign
```

## Next wiring (after key)

1. DEX / Ag screen: venue **Piteas** → quote → preview amounts → approve ERC-20 to router if needed → swap tx.
2. Agent tool: `quote_piteas` (read-only) + propose path that reuses the same approval gate.
3. Optional ignored integration test hitting live `sdk.piteas.io` (mainnet read-only).
