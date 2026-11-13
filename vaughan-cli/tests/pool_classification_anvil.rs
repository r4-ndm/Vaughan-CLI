//! Anvil integration tests for live pool kind classification (V2 vs V3 vs Unknown).
//!
//! Validates:
//! 1. `LiveSwapQuoter::quote_dca` correctly classifies real V3 pool bytecode as `DexSwapKind::V3`.
//! 2. `LiveSwapQuoter::quote_dca` correctly classifies real V2 pair bytecode as `DexSwapKind::V2`.
//! 3. `LiveSwapQuoter::quote_dca` fails closed (`DexSwapKind::Unknown`) for contracts with neither shape.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use alloy::primitives::aliases::{I24, U112, U160};
use alloy::primitives::{address, Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use serde_json::{json, Value};
use vaughan_cli::signerd::{LiveSwapQuoter, SwapQuoter};
use vaughan_core::core::proposal::DexSwapKind;
use vaughan_core::core::wiz4rd::FACTORY_943;

const TEST_CHAIN_ID: u64 = 943;
const ROUTER_943: Address = address!("DaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7");
const TOKEN0: Address = address!("29bab93456c0E97EE931C1554c7C215480aa7766");
const TOKEN1: Address = address!("70499adEBB11Efd915E3b69E700c331778628707"); // tWPLS

sol! {
    interface IPancakeV3Pool {
        function slot0() external view returns (
            uint160 sqrtPriceX96,
            int24 tick,
            uint16 observationIndex,
            uint16 observationCardinality,
            uint16 observationCardinalityNext,
            uint8 feeProtocol,
            bool unlocked
        );
        function liquidity() external view returns (uint128);
        function fee() external view returns (uint24);
        function token0() external view returns (address);
        function token1() external view returns (address);
    }

    interface IPancakeV3Factory {
        function getPool(address tokenA, address tokenB, uint24 fee) external view returns (address pool);
    }

    interface IPairV2 {
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
        function token0() external view returns (address);
        function token1() external view returns (address);
    }

    interface IRouterV2 {
        function getAmountsOut(uint256 amountIn, address[] calldata path) external view returns (uint256[] memory amounts);
    }
}

fn ret_u256(v: u64) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out[24..].copy_from_slice(&v.to_be_bytes());
    out
}

fn ret_address(addr: Address) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out[12..32].copy_from_slice(addr.as_slice());
    out
}

fn ret_amounts_out_pair(amount_in: u64, amount_out: u64) -> Vec<u8> {
    let mut out = vec![0u8; 128];
    out[31] = 0x20;
    out[63] = 0x02;
    out[88..96].copy_from_slice(&amount_in.to_be_bytes());
    out[120..128].copy_from_slice(&amount_out.to_be_bytes());
    out
}

fn assemble_dispatcher(routes: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    fn chunk_store_cost(offset: usize) -> usize {
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
        current_offset += 1 + chunk_bytes + 6;
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

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Anvil {
    child: Child,
    port: u16,
}

impl Anvil {
    fn start() -> Self {
        let port = free_port();
        let child = Command::new("anvil")
            .args(["--port", &port.to_string(), "--chain-id", "943", "--silent"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("anvil must be on PATH (foundry)");
        let anvil = Self { child, port };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if anvil.rpc("eth_chainId", json!([])).is_ok() {
                return anvil;
            }
            if Instant::now() > deadline {
                panic!("anvil did not start in time");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn rpc(&self, method: &str, params: Value) -> Result<Value, String> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let out = Command::new("curl")
            .args(["-s", "-X", "POST", "-H", "Content-Type: application/json"])
            .arg("-d")
            .arg(body.to_string())
            .arg(self.url())
            .output()
            .expect("curl must be available");
        let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
        if let Some(err) = v.get("error") {
            return Err(err.to_string());
        }
        v.get("result")
            .cloned()
            .ok_or_else(|| "missing result field".into())
    }

    fn set_code(&self, address: Address, code: &[u8]) {
        let code_hex = format!("0x{}", hex::encode(code));
        self.rpc("anvil_setCode", json!([format!("{address:#x}"), code_hex]))
            .expect("anvil_setCode must succeed");
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[tokio::test]
async fn test_quoter_classifies_v3_pool_v2_pair_and_unknown() {
    let anvil = Anvil::start();
    let quoter = LiveSwapQuoter;
    let amount_in = U256::from(500_000_000_000_000_000u64); // 0.5 PLS

    let v3_pool = address!("d47e01c1af55a48c11d0e324fb1853cf2501e0dc");
    let v2_pair = address!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    let unknown_pool = address!("cccccccccccccccccccccccccccccccccccccccc");

    // 1. Setup real V3 Pool bytecode and factory on Anvil
    let sqrt = U160::from(U256::from(1u128) << 96);
    let liquidity = 10u128.pow(22);
    let slot0_ret = IPancakeV3Pool::slot0Return {
        sqrtPriceX96: sqrt,
        tick: I24::ZERO,
        observationIndex: 0,
        observationCardinality: 10,
        observationCardinalityNext: 10,
        feeProtocol: 0,
        unlocked: true,
    };
    let slot0_bytes = IPancakeV3Pool::slot0Call::abi_encode_returns(&slot0_ret);
    let liq_bytes = IPancakeV3Pool::liquidityCall::abi_encode_returns(&liquidity);

    let v3_routes = vec![
        (IPancakeV3Pool::feeCall::SELECTOR, ret_u256(500)),
        (IPancakeV3Pool::token0Call::SELECTOR, ret_address(TOKEN0)),
        (IPancakeV3Pool::token1Call::SELECTOR, ret_address(TOKEN1)),
        (IPancakeV3Pool::slot0Call::SELECTOR, slot0_bytes),
        (IPancakeV3Pool::liquidityCall::SELECTOR, liq_bytes),
    ];
    anvil.set_code(v3_pool, &assemble_dispatcher(&v3_routes));

    // Plant factory returning v3_pool for getPool
    let factory_addr: Address = FACTORY_943.parse().unwrap();
    let factory_routes = vec![(
        IPancakeV3Factory::getPoolCall::SELECTOR,
        ret_address(v3_pool),
    )];
    anvil.set_code(factory_addr, &assemble_dispatcher(&factory_routes));

    // 2. Setup real V2 Pair bytecode and V2 router on Anvil
    let reserves_ret = IPairV2::getReservesReturn {
        reserve0: U112::from(10_000_000_000_000_000_000u128),
        reserve1: U112::from(10_000_000_000_000_000_000u128),
        blockTimestampLast: 1_700_000_000,
    };
    let reserves_bytes = IPairV2::getReservesCall::abi_encode_returns(&reserves_ret);
    let v2_routes = vec![
        (IPairV2::getReservesCall::SELECTOR, reserves_bytes),
        (IPairV2::token0Call::SELECTOR, ret_address(TOKEN0)),
        (IPairV2::token1Call::SELECTOR, ret_address(TOKEN1)),
    ];
    anvil.set_code(v2_pair, &assemble_dispatcher(&v2_routes));

    // Plant V2 router returning amounts out
    let router_routes = vec![(
        IRouterV2::getAmountsOutCall::SELECTOR,
        ret_amounts_out_pair(500_000_000_000_000_000, 100_000),
    )];
    anvil.set_code(ROUTER_943, &assemble_dispatcher(&router_routes));

    // 3. Setup contract with neither shape (e.g. dummy return 0x00)
    anvil.set_code(unknown_pool, &[0x00]); // STOP

    // TEST 1: V3 Pool classification
    let quote_v3 = quoter
        .quote_dca(&anvil.url(), TEST_CHAIN_ID, v3_pool, amount_in)
        .await
        .expect("V3 quote_dca must succeed against real V3 pool bytecode");

    match quote_v3.kind {
        DexSwapKind::V3 { pool, fee, quote } => {
            assert_eq!(pool, v3_pool, "classified pool address must match");
            assert_eq!(fee, 500, "classified fee tier must match");
            assert!(quote > U256::ZERO, "re-derived V3 quote must be > 0");
        }
        other => panic!("expected DexSwapKind::V3, got: {other:?}"),
    }
    assert_eq!(quote_v3.fee, Some(500));
    assert!(quote_v3.expected_out > U256::ZERO);

    // TEST 2: V2 Pair classification
    let quote_v2 = quoter
        .quote_dca(&anvil.url(), TEST_CHAIN_ID, v2_pair, amount_in)
        .await
        .expect("V2 quote_dca must succeed against real V2 pair bytecode");

    assert_eq!(
        quote_v2.kind,
        DexSwapKind::V2,
        "expected DexSwapKind::V2 for real V2 pair bytecode"
    );
    assert_eq!(quote_v2.router, ROUTER_943);
    assert_eq!(quote_v2.expected_out, U256::from(100_000));
    assert_eq!(quote_v2.fee, None);

    // TEST 3: Unknown contract shape fails closed
    let quote_unknown = quoter
        .quote_dca(&anvil.url(), TEST_CHAIN_ID, unknown_pool, amount_in)
        .await
        .expect("unknown shape must return Ok(unknown) without panic");

    assert_eq!(
        quote_unknown.kind,
        DexSwapKind::Unknown,
        "contract with neither V2 nor V3 shape must classify as Unknown"
    );
    assert_eq!(quote_unknown.expected_out, U256::ZERO);

    // TEST 4: Address::ZERO fails closed
    let quote_zero = quoter
        .quote_dca(&anvil.url(), TEST_CHAIN_ID, Address::ZERO, amount_in)
        .await
        .expect("Address::ZERO must return Ok(unknown)");

    assert_eq!(
        quote_zero.kind,
        DexSwapKind::Unknown,
        "Address::ZERO must classify as Unknown"
    );
}
