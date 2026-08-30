//! Live PulseChain mainnet (369) QuoterV2 smoke — 9mm WPLS→HEX.
//!
//! Skipped in default CI. Run with:
//! ```sh
//! cargo test -p vaughan-core --test dex_quote_fork -- --ignored --nocapture
//! ```

use alloy::primitives::{address, U256};
use vaughan_core::core::quote_v3_exact_in;

const RPC: &str = "https://rpc.pulsechain.com";
const CHAIN_ID: u64 = 369;
const NINEMM_QUOTER: alloy::primitives::Address =
    address!("0xd6840a5f07d21e68383f159a19a9842af32bdcc5");
const WPLS: alloy::primitives::Address = address!("0xA1077a294dDE1B09bB078844df40758a5D0f9a27");
const HEX: alloy::primitives::Address = address!("0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39");

#[tokio::test]
#[ignore = "live PulseChain mainnet 369 RPC"]
async fn nine_mm_quoter_wpls_to_hex() {
    let amount_in = U256::from(1_000_000_000_000_000_000u64);
    let quote = quote_v3_exact_in(
        RPC,
        CHAIN_ID,
        WPLS,
        HEX,
        amount_in,
        2500,
        Some(NINEMM_QUOTER),
    )
    .await
    .expect("9mm QuoterV2 WPLS→HEX");
    assert!(quote.amount_out > U256::ZERO);
    assert!(quote.amount_out < amount_in);
}
