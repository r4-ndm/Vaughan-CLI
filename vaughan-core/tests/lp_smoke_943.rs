//! Live LP smoke matrix on Pulse testnet 943 (wiz4rd V3).
//!
//! Catches fee-tier mismatches, broken pool discovery, and slow enable checks before
//! anyone walks the TUI manually.
//!
//! ```sh
//! cargo test -p vaughan-core --test lp_smoke_943 -- --ignored --nocapture
//! ```

use std::str::FromStr;
use std::time::Instant;

use alloy::primitives::Address;
use vaughan_core::core::lp_smoke::{LpSmoke943Pair, LP_SMOKE_943, LP_SMOKE_943_VENUE, RPC_943};
use vaughan_core::core::{
    discover_v3_pool_fee_tier, display_price_range_from_preset, fetch_v3_lp_pool_quote,
    v3_lp_token_enable_status, v3_pool_lifecycle, DexVenue, V3LpDeployParams, V3PoolLifecycle,
};

const CHAIN_ID: u64 = 943;
const DEC: u8 = 18;
/// Any valid checksummed EOA — enable check only reads NPM allowances (no broadcast).
const SMOKE_FROM: &str = "0x1111111111111111111111111111111111111111";

fn parse_addr(s: &str) -> Address {
    Address::from_str(s).unwrap_or_else(|_| panic!("invalid smoke address {s}"))
}

fn jim_jane_pair() -> &'static LpSmoke943Pair {
    LP_SMOKE_943
        .iter()
        .find(|p| p.label == "JIM/JANE")
        .expect("JIM/JANE catalog entry")
}

#[tokio::test]
#[ignore = "live PulseChain testnet 943 RPC"]
async fn catalog_pairs_ready_at_catalog_fee() {
    for entry in LP_SMOKE_943 {
        let t0 = parse_addr(entry.token0);
        let t1 = parse_addr(entry.token1);
        assert!(t0 < t1, "{} token0 must sort below token1", entry.label);
        let lifecycle = v3_pool_lifecycle(RPC_943, LP_SMOKE_943_VENUE, CHAIN_ID, t0, t1, entry.fee)
            .await
            .unwrap_or_else(|e| panic!("{} lifecycle @ {}: {e:?}", entry.label, entry.fee));
        assert_eq!(
            lifecycle,
            V3PoolLifecycle::Ready,
            "{} expected Ready at fee {}",
            entry.label,
            entry.fee
        );
    }
}

#[tokio::test]
#[ignore = "live PulseChain testnet 943 RPC"]
async fn discover_finds_catalog_fee_for_each_pair() {
    for entry in LP_SMOKE_943 {
        let t0 = parse_addr(entry.token0);
        let t1 = parse_addr(entry.token1);
        let found = discover_v3_pool_fee_tier(RPC_943, LP_SMOKE_943_VENUE, CHAIN_ID, t0, t1)
            .await
            .expect("discover");
        assert_eq!(found, Some(entry.fee), "{} discover fee", entry.label);
    }
}

#[tokio::test]
#[ignore = "live PulseChain testnet 943 RPC"]
async fn fetch_quote_auto_switches_wrong_default_fee() {
    for entry in LP_SMOKE_943 {
        if entry.tui_default_fee == entry.fee {
            continue;
        }
        let t0 = parse_addr(entry.token0);
        let t1 = parse_addr(entry.token1);
        let quote = fetch_v3_lp_pool_quote(
            RPC_943,
            LP_SMOKE_943_VENUE,
            CHAIN_ID,
            t0,
            t1,
            DEC,
            DEC,
            entry.tui_default_fee,
        )
        .await
        .unwrap_or_else(|e| panic!("{} quote: {e:?}", entry.label));
        assert_eq!(
            quote.suggested_fee_tier,
            Some(entry.fee),
            "{} should suggest catalog fee when TUI default is wrong",
            entry.label
        );
        assert_eq!(
            quote.lifecycle,
            V3PoolLifecycle::Ready,
            "{} should load pool at discovered fee",
            entry.label
        );
        assert!(
            quote.pool_price_token1_per_token0.is_some(),
            "{} should include live pool price",
            entry.label
        );
    }
}

#[tokio::test]
#[ignore = "live PulseChain testnet 943 RPC"]
async fn jim_jane_enable_check_completes_quickly() {
    let entry = jim_jane_pair();
    let t0 = parse_addr(entry.token0);
    let t1 = parse_addr(entry.token1);
    let quote = fetch_v3_lp_pool_quote(
        RPC_943,
        LP_SMOKE_943_VENUE,
        CHAIN_ID,
        t0,
        t1,
        DEC,
        DEC,
        entry.fee,
    )
    .await
    .expect("quote");
    let price = quote.pool_price_token1_per_token0.expect("pool price");
    let center: f64 = price.parse().expect("price f64");
    let (min, max) = display_price_range_from_preset(center, 50.0);
    let params = V3LpDeployParams {
        from: SMOKE_FROM.into(),
        venue: DexVenue::Wiz4rd,
        chain_id: CHAIN_ID,
        rpc_url: RPC_943.into(),
        token0: t0,
        token1: t1,
        fee: entry.fee,
        dec0: DEC,
        dec1: DEC,
        pool_initial_price: price,
        pool_min_price: format!("{min}"),
        pool_max_price: format!("{max}"),
        amount0: "1".into(),
        amount1: "1".into(),
        deposit_on_token0: true,
    };
    let start = Instant::now();
    let status = v3_lp_token_enable_status(&params)
        .await
        .expect("enable status");
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_secs() < 8,
        "enable check took {:?} — one-shot allowance reads must not poll/sleep",
        elapsed
    );
    assert!(status.is_some(), "Ready pool should return enable flags");
}
