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

use vaughan_core::core::vb_browser::cdp_open_url;
use vaughan_core::core::vb_cdp::{
    cdp_read_quote, cdp_select_swap_token, cdp_set_swap_amount, cdp_snapshot, SwapTokenSide,
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
        Some("amount") => cdp_set_swap_amount(cdp, arg(&args, 2)).await,
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

fn arg<'a>(args: &'a [String], i: usize) -> &'a str {
    args.get(i).map(|s| s.as_str()).unwrap_or("")
}
