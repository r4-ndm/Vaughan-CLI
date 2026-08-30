//! Anvil integration tests for browserless DEX quotes (`vaughan_core::core::dex_quote`).
//!
//! Plants mock factory/pool/router bytecode on chain 943 and exercises read-only
//! quote paths — no signing.
//!
//! ```sh
//! cargo test -p vaughan-tui --test dex_quote_anvil -- --nocapture
//! ```

mod common;

use alloy::primitives::{address, aliases::I24, aliases::U160, Address, U256};
use alloy::sol_types::SolCall;
use common::mock_evm::{assemble_dispatcher, plant_code, ret_address, ret_amounts_out_pair};
use common::Anvil;
use std::str::FromStr;
use tokio::runtime::Runtime;
use vaughan_core::core::wiz4rd::FACTORY_943;
use vaughan_core::core::{quote_v2_exact_in, quote_v3_exact_in, quote_v3_path_exact_in};
use wiz4rd_sdk::abi::IPancakeV3Pool;

const MOCK_V2_ROUTER: Address = address!("0x1111111111111111111111111111111111111111");
const MOCK_POOL: Address = address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

/// WZRD < tWPLS (sorted token0/token1 on 943 smoke pair).
const WZRD: Address = address!("0x29bab93456c0E97EE931C1554c7C215480aa7766");
const TWPLS: Address = address!("0x70499adEBB11Efd915E3b69E700c331778628707");
const MEME: Address = address!("0x3333333333333333333333333333333333333333");

const GET_AMOUNTS_OUT: [u8; 4] = [0xd0, 0x6c, 0xa6, 0x1f];
const GET_POOL: [u8; 4] = [0x16, 0x98, 0xee, 0x82];
const SLOT0: [u8; 4] = [0x38, 0x50, 0xc7, 0xbd];
const LIQUIDITY: [u8; 4] = [0x1a, 0x68, 0x65, 0x02];

fn factory_addr() -> Address {
    Address::from_str(FACTORY_943).expect("factory constant")
}

fn slot0_return(sqrt_price_x96: U160) -> Vec<u8> {
    let ret = IPancakeV3Pool::slot0Return {
        sqrtPriceX96: sqrt_price_x96,
        tick: I24::ZERO,
        observationIndex: 0,
        observationCardinality: 10,
        observationCardinalityNext: 10,
        feeProtocol: 0,
        unlocked: true,
    };
    IPancakeV3Pool::slot0Call::abi_encode_returns(&ret)
}

fn liquidity_return(liquidity: u128) -> Vec<u8> {
    IPancakeV3Pool::liquidityCall::abi_encode_returns(&liquidity)
}

fn plant_factory_returns_pool(anvil: &Anvil, pool: Address) {
    plant_code(
        anvil,
        factory_addr(),
        &assemble_dispatcher(&[(GET_POOL, ret_address(pool))]),
    );
}

fn plant_v3_pool(anvil: &Anvil, pool: Address, liquidity: u128) {
    let sqrt = U160::from(U256::from(1u128) << 96);
    plant_code(
        anvil,
        pool,
        &assemble_dispatcher(&[
            (SLOT0, slot0_return(sqrt)),
            (LIQUIDITY, liquidity_return(liquidity)),
        ]),
    );
}

fn plant_wiz4rd_quote_fixture(anvil: &Anvil, liquidity: u128) {
    plant_factory_returns_pool(anvil, MOCK_POOL);
    plant_v3_pool(anvil, MOCK_POOL, liquidity);
}

#[test]
fn anvil_v2_get_amounts_out_quote() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    let amount_in = U256::from(1_000_000u64);
    let expected_out = 777_777u64;

    plant_code(
        &anvil,
        MOCK_V2_ROUTER,
        &assemble_dispatcher(&[(
            GET_AMOUNTS_OUT,
            ret_amounts_out_pair(amount_in.to::<u64>(), expected_out),
        )]),
    );

    let quote = rt
        .block_on(quote_v2_exact_in(
            &anvil.url(),
            MOCK_V2_ROUTER,
            amount_in,
            &[MEME, TWPLS],
        ))
        .expect("v2 quote");
    assert_eq!(quote.amount_out, U256::from(expected_out));
}

#[test]
fn anvil_v3_single_hop_exact_in_quote() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_wiz4rd_quote_fixture(&anvil, 10u128.pow(22));

    let amount_in = U256::from(10u128.pow(17));
    let quote = rt
        .block_on(quote_v3_exact_in(
            &anvil.url(),
            943,
            WZRD,
            TWPLS,
            amount_in,
            500,
            None,
        ))
        .expect("v3 single-hop quote");
    assert!(quote.amount_out > U256::ZERO);
    assert!(quote.amount_out < amount_in);
}

#[test]
fn anvil_v3_two_hop_path_chains_quotes() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_wiz4rd_quote_fixture(&anvil, 10u128.pow(22));

    let amount_in = U256::from(10u128.pow(17));
    let one_hop = rt
        .block_on(quote_v3_exact_in(
            &anvil.url(),
            943,
            WZRD,
            TWPLS,
            amount_in,
            500,
            None,
        ))
        .expect("first hop");

    // Round-trip through the same pool: WZRD → WPLS → WZRD (factory always returns MOCK_POOL).
    let round_trip = rt
        .block_on(quote_v3_path_exact_in(
            &anvil.url(),
            943,
            &[WZRD, TWPLS, WZRD],
            amount_in,
            500,
            None,
        ))
        .expect("two-hop path quote");

    assert!(round_trip.amount_out > U256::ZERO);
    assert!(round_trip.amount_out < one_hop.amount_out);
    assert!(round_trip.amount_out < amount_in);
}

#[test]
fn anvil_v3_quote_errors_when_factory_returns_no_pool() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_code(
        &anvil,
        factory_addr(),
        &assemble_dispatcher(&[(GET_POOL, ret_address(Address::ZERO))]),
    );

    let err = rt
        .block_on(quote_v3_exact_in(
            &anvil.url(),
            943,
            WZRD,
            TWPLS,
            U256::from(1_000u64),
            500,
            None,
        ))
        .expect_err("missing pool");
    let msg = err.user_message().to_lowercase();
    assert!(
        msg.contains("pool") || msg.contains("network"),
        "unexpected: {}",
        err.user_message()
    );
}

#[test]
fn anvil_v3_quote_errors_on_zero_liquidity_pool() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_wiz4rd_quote_fixture(&anvil, 0);

    let err = rt
        .block_on(quote_v3_exact_in(
            &anvil.url(),
            943,
            WZRD,
            TWPLS,
            U256::from(1_000u64),
            500,
            None,
        ))
        .expect_err("zero liquidity");
    assert!(
        err.user_message().contains("liquidity") || err.user_message().contains("quote"),
        "unexpected: {}",
        err.user_message()
    );
}
