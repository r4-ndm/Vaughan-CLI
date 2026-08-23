//! Anvil integration tests for DEX V2 / V3 calldata broadcast.
//!
//! Plants mock routers (selector dispatcher) and sends txs built by
//! [`vaughan_tui::views::dex_calldata`] through the real wallet signer —
//! same builders the DEX view uses.
//!
//! Requires `anvil` and `curl` on PATH:
//! ```sh
//! cargo test -p vaughan-tui --test dex_view -- --nocapture
//! ```

mod common;

use alloy::primitives::{address, Address, U256};
use common::{funded_wallet, Anvil};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use vaughan_tui::views::dex_calldata::{
    build_approve_tx, build_swap_tx, erc20_approve_selector, v2_swap_exact_eth_selector,
    v2_swap_exact_tokens_selector, v3_exact_input_selector, v3_exact_input_single_selector,
    DexProtocol, DexSwapRequest,
};

const MOCK_ROUTER: Address = address!("0x1111111111111111111111111111111111111111");
const MOCK_TOKEN: Address = address!("0x2222222222222222222222222222222222222222");
const TWPLS: Address = address!("0x70499adEBB11Efd915E3b69E700c331778628707");

/// ABI-encode a single `uint256` return.
fn ret_u256(v: u64) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out[24..].copy_from_slice(&v.to_be_bytes());
    out
}

/// ABI-encode `uint256[]` with one element (V2 swap return shape).
fn ret_u256_array_one(v: u64) -> Vec<u8> {
    let mut out = vec![0u8; 96];
    out[31] = 0x20; // offset
    out[63] = 0x01; // length
    out[88..96].copy_from_slice(&v.to_be_bytes());
    out
}

/// Minimal selector dispatcher (same family as agent Anvil fixtures).
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

#[test]
fn anvil_v2_native_swap_broadcasts() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let rt = Runtime::new().unwrap();

    plant_code(
        &anvil,
        MOCK_ROUTER,
        &assemble_dispatcher(&[(v2_swap_exact_eth_selector(), ret_u256_array_one(99))]),
    );

    let from = wallet.active_address().unwrap().to_string();
    let recipient: Address = from.parse().unwrap();
    let tx = build_swap_tx(&DexSwapRequest {
        protocol: DexProtocol::V2,
        router: MOCK_ROUTER,
        token_in: TWPLS,
        token_out: MOCK_TOKEN,
        wpls: Some(TWPLS),
        native_in: true,
        amount_in: U256::from(10u64).pow(U256::from(16)), // 0.01 tPLS
        min_out: U256::from(1u64),
        fee: 3000,
        recipient,
        from: from.clone(),
        chain_id: 943,
    })
    .unwrap();

    let hash = rt
        .block_on(wallet.send_transaction(tx))
        .unwrap_or_else(|e| panic!("v2 native swap failed: {}", e.user_message()));
    wait_receipt(&anvil, &hash.to_string());
}

#[test]
fn anvil_v2_token_swap_after_approve() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let rt = Runtime::new().unwrap();

    plant_code(
        &anvil,
        MOCK_TOKEN,
        &assemble_dispatcher(&[(erc20_approve_selector(), ret_u256(1))]),
    );
    plant_code(
        &anvil,
        MOCK_ROUTER,
        &assemble_dispatcher(&[(v2_swap_exact_tokens_selector(), ret_u256_array_one(42))]),
    );

    let from = wallet.active_address().unwrap().to_string();
    let recipient: Address = from.parse().unwrap();
    let amount = U256::from(1_000_000u64);

    let approve = build_approve_tx(MOCK_TOKEN, MOCK_ROUTER, amount, &from, 943);
    let approve_hash = rt
        .block_on(wallet.send_transaction(approve))
        .unwrap_or_else(|e| panic!("approve failed: {}", e.user_message()));
    wait_receipt(&anvil, &approve_hash.to_string());

    let token_out = address!("0x3333333333333333333333333333333333333333");
    let swap = build_swap_tx(&DexSwapRequest {
        protocol: DexProtocol::V2,
        router: MOCK_ROUTER,
        token_in: MOCK_TOKEN,
        token_out,
        wpls: Some(TWPLS),
        native_in: false,
        amount_in: amount,
        min_out: U256::from(1u64),
        fee: 3000,
        recipient,
        from,
        chain_id: 943,
    })
    .unwrap();

    // meme→meme path is token → WPLS → out (3 hops) — still uses tokens-for-tokens.
    let data = hex::decode(swap.data.as_ref().unwrap().trim_start_matches("0x")).unwrap();
    assert_eq!(&data[..4], &v2_swap_exact_tokens_selector());

    let hash = rt
        .block_on(wallet.send_transaction(swap))
        .unwrap_or_else(|e| panic!("v2 token swap failed: {}", e.user_message()));
    wait_receipt(&anvil, &hash.to_string());
}

#[test]
fn anvil_v3_exact_input_single_broadcasts() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let rt = Runtime::new().unwrap();

    plant_code(
        &anvil,
        MOCK_ROUTER,
        &assemble_dispatcher(&[(v3_exact_input_single_selector(), ret_u256(77))]),
    );

    let from = wallet.active_address().unwrap().to_string();
    let recipient: Address = from.parse().unwrap();
    let tx = build_swap_tx(&DexSwapRequest {
        protocol: DexProtocol::V3,
        router: MOCK_ROUTER,
        token_in: TWPLS,
        token_out: MOCK_TOKEN,
        wpls: Some(TWPLS),
        native_in: true,
        amount_in: U256::from(10u64).pow(U256::from(15)),
        min_out: U256::from(1u64),
        fee: 3000,
        recipient,
        from,
        chain_id: 943,
    })
    .unwrap();

    let data = hex::decode(tx.data.as_ref().unwrap().trim_start_matches("0x")).unwrap();
    assert_eq!(&data[..4], &v3_exact_input_single_selector());

    let hash = rt
        .block_on(wallet.send_transaction(tx))
        .unwrap_or_else(|e| panic!("v3 single failed: {}", e.user_message()));
    wait_receipt(&anvil, &hash.to_string());
}

#[test]
fn anvil_v3_exact_input_multihop_broadcasts() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let rt = Runtime::new().unwrap();

    plant_code(
        &anvil,
        MOCK_TOKEN,
        &assemble_dispatcher(&[(erc20_approve_selector(), ret_u256(1))]),
    );
    plant_code(
        &anvil,
        MOCK_ROUTER,
        &assemble_dispatcher(&[(v3_exact_input_selector(), ret_u256(88))]),
    );

    let from = wallet.active_address().unwrap().to_string();
    let recipient: Address = from.parse().unwrap();
    let amount = U256::from(5_000u64);
    let token_out = address!("0x3333333333333333333333333333333333333333");

    let approve = build_approve_tx(MOCK_TOKEN, MOCK_ROUTER, amount, &from, 943);
    let ah = rt.block_on(wallet.send_transaction(approve)).unwrap();
    wait_receipt(&anvil, &ah.to_string());

    let swap = build_swap_tx(&DexSwapRequest {
        protocol: DexProtocol::V3,
        router: MOCK_ROUTER,
        token_in: MOCK_TOKEN,
        token_out,
        wpls: Some(TWPLS),
        native_in: false,
        amount_in: amount,
        min_out: U256::from(1u64),
        fee: 3000,
        recipient,
        from,
        chain_id: 943,
    })
    .unwrap();

    let data = hex::decode(swap.data.as_ref().unwrap().trim_start_matches("0x")).unwrap();
    assert_eq!(&data[..4], &v3_exact_input_selector());

    let hash = rt
        .block_on(wallet.send_transaction(swap))
        .unwrap_or_else(|e| panic!("v3 multihop failed: {}", e.user_message()));
    wait_receipt(&anvil, &hash.to_string());
}
