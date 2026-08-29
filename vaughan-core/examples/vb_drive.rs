//! Live VB driver for manual verification against a running
//! `vaughan-dapp-browser` (CDP on 127.0.0.1:9222).
//!
//! Dev aid only — lets agents iterate on `vb_cdp` flows (token pickers,
//! amount typing, quote reads) without restarting MCP host sessions.
//!
//! Usage:
//!   cargo run -p vaughan-core --example vb_drive -- select-in PLS
//!   cargo run -p vaughan-core --example vb_drive -- select-out USDC
//!   cargo run -p vaughan-core --example vb_drive -- amount 1000000
//!   cargo run -p vaughan-core --example vb_drive -- quote [USDC]

use alloy::primitives::{Address, U256};
use vaughan_core::chains::evm::tokens_for_chain;
use vaughan_core::core::aggregator::{quote_aggregator, AggQuoteRequest, AggVenue};
use vaughan_core::core::vb_browser::cdp_open_url;
use vaughan_core::core::vb_cdp::{
    self, cdp_read_quote, cdp_select_swap_token, cdp_snapshot, SwapTokenSide,
};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cdp = "http://127.0.0.1:9222";
    let out = match args.get(1).map(|s| s.as_str()) {
        Some("open") => cdp_open_url(cdp, arg(&args, 2))
            .await
            .map(|id| serde_json::json!({ "ok": true, "target": id })),
        Some("select-in") => cdp_select_swap_token(cdp, arg(&args, 2), SwapTokenSide::Input).await,
        Some("select-out") => {
            cdp_select_swap_token(cdp, arg(&args, 2), SwapTokenSide::Output).await
        }
        Some("amount") => {
            let strategy = args
                .get(3)
                .and_then(|s| vb_cdp::TypeStrategy::parse(s))
                .unwrap_or_default();
            vb_cdp::cdp_set_swap_amount_with_strategy(cdp, arg(&args, 2), strategy).await
        }
        // Oracle unit-price probe — same path as browser_read_quote's
        // sell_check: 1k units of SYM → USDC via EmpX, dead-address recipient.
        Some("price") => {
            let sym = arg(&args, 2);
            let registry = tokens_for_chain(369);
            let (addr, native, decimals) = if sym.eq_ignore_ascii_case("pls") {
                (Address::ZERO, true, 18u8)
            } else {
                match registry.iter().find(|t| t.symbol.eq_ignore_ascii_case(sym)) {
                    Some(t) => (
                        t.address.parse::<Address>().unwrap_or(Address::ZERO),
                        false,
                        t.decimals,
                    ),
                    None => {
                        eprintln!("unknown symbol {sym}");
                        std::process::exit(2);
                    }
                }
            };
            let usdc: Address = registry
                .iter()
                .find(|t| t.symbol == "USDC")
                .and_then(|t| t.address.parse().ok())
                .unwrap_or(Address::ZERO);
            let dead: Address = "0x000000000000000000000000000000000000dEaD"
                .parse()
                .unwrap_or(Address::ZERO);
            let req = AggQuoteRequest {
                token_in: addr,
                token_out: usdc,
                token_in_is_native: native,
                token_out_is_native: false,
                amount_in: U256::from(1000u128 * 10u128.pow(decimals as u32)),
                slippage_percent: 0.5,
                account: Some(dead),
            };
            quote_aggregator(AggVenue::Empseal, &req, 369, None, None)
                .await
                .map(|q| {
                    let out: f64 = q.amount_out.to_string().parse().unwrap_or(0.0);
                    serde_json::json!({ "ok": true, "unit_price_usd": out / 1e6 / 1000.0 })
                })
        }
        Some("snap") => cdp_snapshot(cdp).await,
        Some("quote") => cdp_read_quote(cdp, args.get(2).map(|s| s.as_str())).await,
        _ => {
            eprintln!(
                "usage: vb_drive <open URL | select-in SYM | select-out SYM | amount N | snap | quote [SYM]>"
            );
            std::process::exit(2);
        }
    };
    match out {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn arg(args: &[String], i: usize) -> &str {
    args.get(i).map(|s| s.as_str()).unwrap_or("")
}
