//! Shared wiz4rd V3 LP mock fixtures for Anvil integration tests (943).

use alloy::primitives::{address, aliases::I24, aliases::U160, Address, U256};
use alloy::sol_types::SolCall;
use std::str::FromStr;
use std::time::{Duration, Instant};

use super::mock_evm::{assemble_dispatcher, plant_code, ret_address, ret_u256, ret_u8};
use super::Anvil;
use vaughan_core::core::wiz4rd::{FACTORY_943, POSITION_MANAGER_943};
use vaughan_core::core::{DexVenue, V3LpDeployParams};
use wiz4rd_sdk::abi::{IPancakeV3Factory, IPancakeV3Pool};

pub const MOCK_POOL: Address = address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
pub const TOKEN0: Address = address!("0x29bab93456c0E97EE931C1554c7C215480aa7766"); // WZRD
pub const TOKEN1: Address = address!("0x70499adEBB11Efd915E3b69E700c331778628707"); // tWPLS
pub const NPM: Address = address!("0xf1b1D004dD8bFC618F977F6ACAD127a60c566745");

const GET_POOL: [u8; 4] = [0x16, 0x98, 0xee, 0x82];
const SLOT0: [u8; 4] = [0x38, 0x50, 0xc7, 0xbd];
const LIQUIDITY: [u8; 4] = [0x1a, 0x68, 0x65, 0x02];
const ALLOWANCE: [u8; 4] = [0xdd, 0x62, 0xed, 0x3e];
const APPROVE: [u8; 4] = [0x09, 0x5e, 0xa7, 0xb3];
const BALANCE_OF: [u8; 4] = [0x70, 0xa0, 0x82, 0x31];
const DECIMALS: [u8; 4] = [0x31, 0x3c, 0xe5, 0x67];
const MINT: [u8; 4] = [0x88, 0x31, 0x64, 0x56];

pub fn factory_addr() -> Address {
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

/// Factory `getPool` → `pool`; optional `createPool` success return.
pub fn plant_factory(anvil: &Anvil, get_pool: Address, with_create: bool) {
    let mut routes = vec![(GET_POOL, ret_address(get_pool))];
    if with_create {
        routes.push((
            IPancakeV3Factory::createPoolCall::SELECTOR,
            ret_address(MOCK_POOL),
        ));
    }
    plant_code(anvil, factory_addr(), &assemble_dispatcher(&routes));
}

pub fn plant_factory_missing(anvil: &Anvil) {
    plant_factory(anvil, Address::ZERO, false);
}

pub fn plant_factory_missing_with_create(anvil: &Anvil) {
    plant_factory(anvil, Address::ZERO, true);
}

pub fn plant_factory_pool(anvil: &Anvil, pool: Address) {
    plant_factory(anvil, pool, false);
}

pub fn plant_v3_pool(
    anvil: &Anvil,
    pool: Address,
    sqrt: U160,
    liquidity: u128,
    with_initialize: bool,
) {
    let mut routes = vec![
        (SLOT0, slot0_return(sqrt)),
        (LIQUIDITY, liquidity_return(liquidity)),
    ];
    if with_initialize {
        routes.push((IPancakeV3Pool::initializeCall::SELECTOR, Vec::new()));
    }
    plant_code(anvil, pool, &assemble_dispatcher(&routes));
}

pub fn plant_v3_pool_uninitialized(anvil: &Anvil) {
    plant_v3_pool(anvil, MOCK_POOL, U160::ZERO, 0, true);
}

pub fn plant_ready_pool_fixture(anvil: &Anvil) {
    let sqrt = U160::from(U256::from(1u128) << 96);
    plant_factory_pool(anvil, MOCK_POOL);
    plant_v3_pool(anvil, MOCK_POOL, sqrt, 10u128.pow(22), false);
}

pub fn plant_erc20_allowance(anvil: &Anvil, token: Address, allowance: u64) {
    plant_code(
        anvil,
        token,
        &assemble_dispatcher(&[
            (DECIMALS, ret_u8(18)),
            (BALANCE_OF, ret_u256(u64::MAX)),
            (ALLOWANCE, ret_u256(allowance)),
        ]),
    );
}

pub fn plant_erc20_with_approve(anvil: &Anvil, token: Address, allowance: u64) {
    plant_code(
        anvil,
        token,
        &assemble_dispatcher(&[
            (DECIMALS, ret_u8(18)),
            (BALANCE_OF, ret_u256(u64::MAX)),
            (ALLOWANCE, ret_u256(allowance)),
            (APPROVE, Vec::new()),
        ]),
    );
}

pub fn plant_npm_mint(anvil: &Anvil) {
    plant_code(anvil, NPM, &assemble_dispatcher(&[(MINT, Vec::new())]));
}

pub fn deploy_params(anvil: &Anvil, from: &str) -> V3LpDeployParams {
    V3LpDeployParams {
        from: from.to_string(),
        venue: DexVenue::Wiz4rd,
        chain_id: 943,
        rpc_url: anvil.url(),
        token0: TOKEN0,
        token1: TOKEN1,
        fee: 500,
        dec0: 18,
        dec1: 18,
        pool_initial_price: "1".into(),
        pool_min_price: String::new(),
        pool_max_price: String::new(),
        amount0: "1".into(),
        amount1: String::new(),
        deposit_on_token0: true,
    }
}

pub fn wait_receipt(anvil: &Anvil, hash: &str) {
    use serde_json::json;
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

pub fn npm_catalog_matches() {
    assert_eq!(
        NPM,
        Address::from_str(POSITION_MANAGER_943).unwrap(),
        "NPM must match wiz4rd deploy + allowlist"
    );
    assert!(TOKEN0 < TOKEN1, "Uni V3 token0 must sort below token1");
}
