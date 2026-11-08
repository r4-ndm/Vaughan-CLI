//! Live LibertySwap quote probe — hits the real API, so it is `#[ignore]`d.
//!
//! ```text
//! cargo test -p vaughan-hl --test liberty_quote_live -- --ignored --nocapture
//! ```

use alloy::primitives::{Address, U256};
use std::str::FromStr;

#[tokio::test]
#[ignore = "hits the live LibertySwap API"]
async fn liberty_quotes_arbitrum_usdc_to_btc_meme_on_pulsechain() {
    let amount = U256::from(10_000_000u64); // 10 USDC (6 decimals)
    let recipient = Address::from_str("0xfe4770adcedcbfe2b73bdb69f83321f1f79bfd63").unwrap();

    let quote = vaughan_hl::siphon::quote_liberty_bridge(amount, recipient)
        .await
        .expect("live LibertySwap quote should succeed");

    println!(
        "router={:#x} usdc={:#x} target={:#x} amount_in={} dest={} fee={:.2}%",
        quote.router,
        quote.usdc,
        quote.target,
        quote.amount_usdc,
        quote.dest_amount,
        quote.fee_percent
    );

    assert_eq!(quote.amount_usdc, amount);
    assert!(
        !quote.dest_amount.is_zero(),
        "destination amount must be non-zero"
    );
    assert!(
        !quote.bridge_calldata.is_empty(),
        "bridge calldata must be present"
    );
    assert!(
        !quote.approval_calldata.is_empty(),
        "approval calldata must be present"
    );
}

/// End-to-end: a profitable epoch must produce a `Quoted` receipt sized in USDC and
/// leave `data/siphon_pending.json` behind. Run single-threaded (it changes cwd):
/// ```text
/// cargo test -p vaughan-hl --test liberty_quote_live -- --ignored --nocapture --test-threads=1
/// ```
#[tokio::test]
#[ignore = "hits the live LibertySwap API"]
async fn profitable_epoch_writes_pending_execution() {
    use vaughan_hl::market_data::LiveTradeFill;
    use vaughan_hl::siphon::{pending_exec_path, SiphonManager, SiphonStatus};
    use vaughan_hl::strategy::AutoReinvestConfig;

    let dir = std::env::temp_dir().join(format!("vaughan-siphon-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_current_dir(&dir).unwrap();

    let recipient = Address::from_str("0xfe4770adcedcbfe2b73bdb69f83321f1f79bfd63").unwrap();
    let config = AutoReinvestConfig::default(); // 20%, weekly, $25 threshold
    let mut mgr = SiphonManager::new_with_recipient(config, 0, recipient);

    let fills = vec![LiveTradeFill {
        oid: 1,
        coin: "BTC".into(),
        dir: "Close Long".into(),
        px: 70_000.0,
        sz: 0.1,
        closed_pnl: 500.0,
        fee: 2.0,
        time_ms: 1_000,
    }];

    // Weekly cadence needs an epoch boundary; now_ms far beyond the start triggers it.
    let now_ms = 8 * 86_400_000u64;
    let receipt = mgr
        .evaluate_and_siphon(&fills, now_ms)
        .await
        .expect("evaluate")
        .expect("receipt");

    println!(
        "status={:?} siphon_usd={:.2} out={:?}",
        receipt.status, receipt.siphon_usd, receipt.amount_out_tokens
    );
    assert!(matches!(receipt.status, SiphonStatus::Quoted));
    // 20% of $498 net => $99.60 => 99_600_000 USDC units
    let intent = receipt.intent.expect("intent");
    assert_eq!(intent.amount_in, U256::from(99_600_000u64));
    assert_eq!(intent.chain_id, 42161);

    let path = pending_exec_path();
    assert!(path.exists(), "pending execution file must exist");
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"amount_usdc\""));
    println!("pending file: {} bytes", body.len());

    std::fs::remove_dir_all(&dir).ok();
}
