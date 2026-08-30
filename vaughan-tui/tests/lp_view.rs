//! Anvil integration tests for wiz4rd V3 LP calldata (same builders as [`LpView`]).
//!
//! Plants mock NPM bytecode at the catalogued wiz4rd address on chain 943 and
//! broadcasts txs built by `vaughan_core::core::dex_lp`.
//!
//! Requires `anvil` and `curl` on PATH:
//! ```sh
//! cargo test -p vaughan-tui --test lp_view -- --nocapture
//! ```

mod common;

use alloy::primitives::{address, keccak256, Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use common::{funded_wallet, Anvil};
use serde_json::json;
use std::str::FromStr;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::wiz4rd::{POSITION_MANAGER_943, WPLS_943, WZRD_SMOKE_943};
use vaughan_core::core::{
    build_v3_collect_evm, build_v3_mint_evm, default_full_range_ticks, list_v3_lp_positions,
    min_out_after_slippage, DEFAULT_DEX_SLIPPAGE_BPS,
};
use wiz4rd_sdk::abi::INonfungiblePositionManager;

const NPM: Address = address!("0xf1b1D004dD8bFC618F977F6ACAD127a60c566745");
const TOKEN0: Address = address!("0x29bab93456c0E97EE931C1554c7C215480aa7766"); // WZRD
const TOKEN1: Address = address!("0x70499adEBB11Efd915E3b69E700c331778628707"); // tWPLS

const OWNER_OF: [u8; 4] = [0x63, 0x52, 0x21, 0x1e];
const POSITIONS: [u8; 4] = [0x99, 0xfb, 0xab, 0x88];
const MINT: [u8; 4] = [0x88, 0x31, 0x64, 0x56];
const COLLECT: [u8; 4] = [0xfc, 0x6f, 0x78, 0x65];

fn ret_address(addr: Address) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out[12..32].copy_from_slice(addr.as_slice());
    out
}

fn ret_u256_pair(a: u64, b: u64) -> Vec<u8> {
    let mut out = vec![0u8; 64];
    out[24..32].copy_from_slice(&a.to_be_bytes());
    out[56..64].copy_from_slice(&b.to_be_bytes());
    out
}

fn ret_positions(token0: Address, token1: Address, liquidity: u128) -> Vec<u8> {
    use alloy::primitives::aliases::{I24, U24, U96};
    let ret = INonfungiblePositionManager::positionsReturn {
        nonce: U96::ZERO,
        operator: Address::ZERO,
        token0,
        token1,
        fee: U24::try_from(500u32).unwrap(),
        tickLower: I24::try_from(-887220i32).unwrap(),
        tickUpper: I24::try_from(887220i32).unwrap(),
        liquidity,
        feeGrowthInside0LastX128: U256::ZERO,
        feeGrowthInside1LastX128: U256::ZERO,
        tokensOwed0: 0,
        tokensOwed1: 0,
    };
    INonfungiblePositionManager::positionsCall::abi_encode_returns(&ret)
}

/// Minimal selector dispatcher (same family as [`super::dex_view`] / agent fixtures).
fn assemble_dispatcher(routes: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    fn chunk_store_cost(offset: usize) -> usize {
        // PUSH32 + (PUSH1|PUSH2 offset) + MSTORE
        1 + 32 + if offset <= 255 { 2 } else { 3 } + 1
    }

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
        let chunk_bytes: usize = (0..chunks).map(|c| chunk_store_cost(c * 32)).sum();
        current_offset += 1 + chunk_bytes + 6; // JUMPDEST + RETURN tail
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
            let off = c * 32;
            if off <= 255 {
                bytecode.push(0x60);
                bytecode.push(off as u8);
            } else {
                bytecode.push(0x61);
                bytecode.push((off >> 8) as u8);
                bytecode.push((off & 0xff) as u8);
            }
            bytecode.push(0x52);
        }
        let size = ret_data.len() as u16;
        bytecode.push(0x61);
        bytecode.push((size >> 8) as u8);
        bytecode.push((size & 0xff) as u8);
        bytecode.extend_from_slice(&[0x60, 0x00, 0xf3]);
    }
    bytecode
}

/// LOG4 `Transfer(from=0, to=owner, tokenId)` then fall through (no RETURN).
fn transfer_log4_prefix(owner: Address, token_id: u8) -> Vec<u8> {
    let topic0 = keccak256("Transfer(address,address,uint256)");
    let mut owner_word = [0u8; 32];
    owner_word[12..32].copy_from_slice(owner.as_slice());
    let mut code = Vec::new();
    code.extend_from_slice(&[0x60, token_id]); // topic3
    code.push(0x7f);
    code.extend_from_slice(&owner_word); // topic2 = to
    code.extend_from_slice(&[0x60, 0x00]); // topic1 = from
    code.push(0x7f);
    code.extend_from_slice(topic0.as_slice());
    code.extend_from_slice(&[0x60, 0x00, 0x60, 0x00, 0xa4]); // LOG4
    code
}

/// LOG4 `Transfer(from=0, to=owner, tokenId)` — empty calldata poke only.
fn transfer_log4_runtime(owner: Address, token_id: u8) -> Vec<u8> {
    let mut code = transfer_log4_prefix(owner, token_id);
    code.extend_from_slice(&[0x60, 0x00, 0x60, 0x00, 0xf3]);
    code
}

fn plant_list_read_mock(owner: Address) -> Vec<u8> {
    assemble_dispatcher(&[
        (OWNER_OF, ret_address(owner)),
        (POSITIONS, ret_positions(TOKEN0, TOKEN1, 1_000)),
        (MINT, Vec::new()),
        (COLLECT, ret_u256_pair(1, 2)),
    ])
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
fn anvil_v3_lp_mint_broadcasts() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let rt = Runtime::new().unwrap();

    plant_code(&anvil, NPM, &assemble_dispatcher(&[(MINT, Vec::new())]));

    let amount0 = U256::from(1_000_000u64);
    let amount1 = U256::from(1_000_000u64);
    let (tick_lower, tick_upper) = default_full_range_ticks(500).unwrap();
    let tx = build_v3_mint_evm(
        wallet.active_address().unwrap(),
        943,
        &anvil.url(),
        TOKEN0,
        TOKEN1,
        500,
        tick_lower,
        tick_upper,
        amount0,
        amount1,
        min_out_after_slippage(amount0, DEFAULT_DEX_SLIPPAGE_BPS),
        min_out_after_slippage(amount1, DEFAULT_DEX_SLIPPAGE_BPS),
        None,
    )
    .unwrap();
    assert_eq!(
        tx.to.to_lowercase(),
        POSITION_MANAGER_943.to_lowercase(),
        "mint must target catalog NPM"
    );

    let hash = rt
        .block_on(wallet.send_transaction(tx))
        .unwrap_or_else(|e| panic!("mint failed: {}", e.user_message()));
    wait_receipt(&anvil, &hash.to_string());
}

#[test]
fn anvil_v3_lp_collect_broadcasts() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let rt = Runtime::new().unwrap();

    plant_code(
        &anvil,
        NPM,
        &assemble_dispatcher(&[(COLLECT, ret_u256_pair(5, 7))]),
    );

    let tx = build_v3_collect_evm(
        wallet.active_address().unwrap(),
        943,
        &anvil.url(),
        U256::from(1u64),
        None,
        u128::MAX,
        u128::MAX,
    )
    .unwrap();

    let hash = rt
        .block_on(wallet.send_transaction(tx))
        .unwrap_or_else(|e| panic!("collect failed: {}", e.user_message()));
    wait_receipt(&anvil, &hash.to_string());
}

#[test]
fn anvil_list_v3_lp_positions_after_transfer_log() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let rt = Runtime::new().unwrap();
    let owner: Address = wallet.active_address().unwrap().parse().unwrap();

    plant_code(&anvil, NPM, &transfer_log4_runtime(owner, 1));

    let poke = EvmTransaction {
        from: wallet.active_address().unwrap().to_string(),
        to: format!("{NPM:#x}"),
        value: "0".into(),
        data: Some("0x".into()),
        gas_limit: Some(200_000),
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id: 943,
    };
    let hash = rt
        .block_on(wallet.send_transaction(poke))
        .expect("emit transfer");
    wait_receipt(&anvil, &hash.to_string());
    let receipt = anvil
        .rpc("eth_getTransactionReceipt", json!([hash.to_string()]))
        .expect("receipt");
    assert!(
        receipt["logs"].as_array().is_some_and(|logs| !logs.is_empty()),
        "poke must emit Transfer log: {receipt}"
    );

    plant_code(&anvil, NPM, &plant_list_read_mock(owner));

    let provider = ProviderBuilder::new().connect_http(anvil.url().parse().unwrap());
    let pos_call = INonfungiblePositionManager::positionsCall {
        tokenId: U256::from(1u64),
    };
    let raw = rt
        .block_on(async {
            provider
                .call(
                    TransactionRequest::default()
                        .to(NPM)
                        .input(pos_call.abi_encode().into()),
                )
                .await
        })
        .expect("positions eth_call");
    assert_eq!(
        raw.len(),
        ret_positions(TOKEN0, TOKEN1, 1_000).len(),
        "mock NPM must return full positions tuple"
    );

    let rows = rt
        .block_on(list_v3_lp_positions(
            &anvil.url(),
            943,
            owner,
            None,
            None,
        ))
        .unwrap_or_else(|e| panic!("list positions: {}", e.user_message()));
    assert_eq!(rows.len(), 1, "expected one LP NFT");
    assert_eq!(rows[0].token_id, U256::from(1u64));
    assert_eq!(rows[0].token0, TOKEN0);
    assert_eq!(rows[0].token1, TOKEN1);
}

#[test]
fn positions_mock_return_decodes() {
    let raw = ret_positions(TOKEN0, TOKEN1, 1_000);
    assert!(
        raw.len() >= 384,
        "positions return must be at least 384 bytes, got {}",
        raw.len()
    );
    let decoded = INonfungiblePositionManager::positionsCall::abi_decode_returns(&raw)
        .expect("positions return roundtrip");
    assert_eq!(decoded.token0, TOKEN0);
    assert_eq!(decoded.token1, TOKEN1);
    assert_eq!(decoded.liquidity, 1_000);
}

#[test]
fn catalog_npm_address_matches_wiz4rd_943() {
    assert_eq!(
        NPM,
        Address::from_str(POSITION_MANAGER_943).unwrap(),
        "test NPM must match wiz4rd deploy + allowlist"
    );
    assert!(TOKEN0 < TOKEN1, "Uni V3 token0 must sort below token1");
    let _ = (WPLS_943, WZRD_SMOKE_943);
}
