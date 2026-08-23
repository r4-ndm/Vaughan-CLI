//! Anvil tests for Aggregator (Ag) / SquirrelSwap broadcast path.
//!
//! Brain HTTP stays out of CI — we replay a real `/swap` response shape
//! ([`vaughan_core::core::aggregator`] fixture), plant a mock router at the
//! returned `tx.to`, and send through the same `EvmTransaction` layout the
//! Ag view builds. Proves approve → swap without calling `api.squirrelswap.pro`.
//!
//! ```sh
//! cargo test -p vaughan-tui --test ag_view -- --nocapture
//! ```

mod common;

use alloy::primitives::{address, Address, Bytes, U256};
use common::{funded_wallet, Anvil};
use serde_json::json;
use std::str::FromStr;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::{AggExecTx, AggQuote, AggVenue};
use vaughan_tui::views::dex_calldata::{build_approve_tx, erc20_approve_selector};

/// Router from the SquirrelSwap Brain `/swap` fixture (see squirrelswap unit test).
const SQUIRREL_ROUTER: Address = address!("0xDa8953Fc615d6E816b9647Afd5536123dcE70B78");
/// WPLS mainnet — used as ERC-20 in for the approve+swap case.
const WPLS_369: Address = address!("0xA1077a294dDE1B09bB078844df40758a5D0f9a27");

/// Selector from fixture calldata `0xc563dcec…`.
fn squirrel_fixture_selector() -> [u8; 4] {
    [0xc5, 0x63, 0xdc, 0xec]
}

fn ret_u256(v: u64) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out[24..].copy_from_slice(&v.to_be_bytes());
    out
}

fn assemble_dispatcher(routes: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut bytecode = vec![0x60, 0x00, 0x35, 0x60, 0xe0, 0x1c];
    let dispatch_size = bytecode.len() + routes.len() * 11 + 5;
    let mut handlers = Vec::new();
    let mut current_offset = dispatch_size;

    for (_sel, ret_data) in routes {
        let handler_target = current_offset as u16;
        handlers.push((handler_target, ret_data.clone()));
        let chunks = if ret_data.is_empty() {
            0
        } else {
            ret_data.len().div_ceil(32)
        };
        current_offset += 1 + chunks * 36 + 6;
    }

    for (i, (sel, _)) in routes.iter().enumerate() {
        let (target, _) = handlers[i];
        bytecode.push(0x80);
        bytecode.push(0x63);
        bytecode.extend_from_slice(sel);
        bytecode.push(0x14);
        bytecode.push(0x61);
        bytecode.push((target >> 8) as u8);
        bytecode.push((target & 0xff) as u8);
        bytecode.push(0x57);
    }
    bytecode.extend_from_slice(&[0x60, 0x00, 0x60, 0x00, 0xfd]);

    for (target, ret_data) in handlers {
        assert_eq!(bytecode.len(), target as usize);
        bytecode.push(0x5b);
        let chunks = if ret_data.is_empty() {
            0
        } else {
            ret_data.len().div_ceil(32)
        };
        for c in 0..chunks {
            let start = c * 32;
            let end = (start + 32).min(ret_data.len());
            let mut word = [0u8; 32];
            word[..end - start].copy_from_slice(&ret_data[start..end]);
            bytecode.push(0x7f);
            bytecode.extend_from_slice(&word);
            bytecode.push(0x60);
            bytecode.push((c * 32) as u8);
            bytecode.push(0x52);
        }
        let size = ret_data.len() as u8;
        bytecode.extend_from_slice(&[0x60, size, 0x60, 0x00, 0xf3]);
    }
    bytecode
}

fn plant_code(anvil: &Anvil, at: Address, code: &[u8]) {
    anvil
        .rpc(
            "anvil_setCode",
            json!([format!("{at:#x}"), format!("0x{}", hex::encode(code))]),
        )
        .expect("anvil_setCode");
}

fn wait_receipt(anvil: &Anvil, hash: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(r) = anvil.rpc("eth_getTransactionReceipt", json!([hash])) {
            if !r.is_null() {
                let status = r["status"].as_str().unwrap_or("0x0");
                assert_eq!(status, "0x1", "tx reverted: {r}");
                return;
            }
        }
        if Instant::now() > deadline {
            panic!("no receipt for {hash}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Same JSON shape as `SquirrelSwapClient` unit fixture → [`AggQuote`].
fn squirrel_fixture_quote(amount_in: U256, native_value: bool) -> AggQuote {
    let data = Bytes::from(vec![0xc5, 0x63, 0xdc, 0xec]);
    let value = if native_value {
        U256::from_str("1003000000000000000").unwrap()
    } else {
        U256::ZERO
    };
    AggQuote {
        venue: AggVenue::SquirrelSwap,
        amount_in,
        amount_out: U256::from_str("15683509506112").unwrap(),
        gas_estimate: None,
        tx: AggExecTx {
            to: SQUIRREL_ROUTER,
            data,
            value,
        },
        spender: SQUIRREL_ROUTER,
    }
}

/// Mirrors [`vaughan_tui::views::ag::AgView`] swap confirm → `EvmTransaction`.
fn ag_swap_tx(quote: &AggQuote, from: &str, chain_id: u64) -> EvmTransaction {
    let data_hex = format!("0x{}", hex::encode(quote.tx.data.as_ref()));
    EvmTransaction {
        from: from.to_string(),
        to: format!("{:#x}", quote.tx.to),
        value: quote.tx.value.to_string(),
        data: Some(data_hex),
        gas_limit: None,
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id,
    }
}

#[test]
fn anvil_squirrel_native_swap_from_fixture_broadcasts() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let rt = Runtime::new().unwrap();

    plant_code(
        &anvil,
        SQUIRREL_ROUTER,
        &assemble_dispatcher(&[(squirrel_fixture_selector(), Vec::new())]),
    );

    let from = wallet.active_address().unwrap().to_string();
    let amount_in = U256::from_str("1000000000000000000").unwrap();
    let quote = squirrel_fixture_quote(amount_in, true);
    let tx = ag_swap_tx(&quote, &from, 943);

    assert_eq!(tx.to.to_lowercase(), format!("{SQUIRREL_ROUTER:#x}"));
    assert_eq!(tx.value, "1003000000000000000");

    let hash = rt
        .block_on(wallet.send_transaction(tx))
        .unwrap_or_else(|e| panic!("squirrel native swap failed: {}", e.user_message()));
    wait_receipt(&anvil, &hash.to_string());
}

#[test]
fn anvil_squirrel_token_approve_then_swap_broadcasts() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let rt = Runtime::new().unwrap();

    plant_code(
        &anvil,
        WPLS_369,
        &assemble_dispatcher(&[(erc20_approve_selector(), ret_u256(1))]),
    );
    plant_code(
        &anvil,
        SQUIRREL_ROUTER,
        &assemble_dispatcher(&[(squirrel_fixture_selector(), Vec::new())]),
    );

    let from = wallet.active_address().unwrap().to_string();
    let amount_in = U256::from_str("1000000000000000000").unwrap();
    let quote = squirrel_fixture_quote(amount_in, false);

    let approve = build_approve_tx(WPLS_369, quote.spender, quote.amount_in, &from, 943);
    let ah = rt
        .block_on(wallet.send_transaction(approve))
        .unwrap_or_else(|e| panic!("squirrel approve failed: {}", e.user_message()));
    wait_receipt(&anvil, &ah.to_string());

    let swap = ag_swap_tx(&quote, &from, 943);
    let hash = rt
        .block_on(wallet.send_transaction(swap))
        .unwrap_or_else(|e| panic!("squirrel token swap failed: {}", e.user_message()));
    wait_receipt(&anvil, &hash.to_string());
}
