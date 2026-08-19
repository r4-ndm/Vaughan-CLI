//! End-to-end Anvil integration tests for `vaughan_core::browser` (`wiz4rd-engine`).
//!
//! Tests capability fingerprinting, PUSH4 selector extraction, dynamic calls,
//! and factory pair discovery against local Anvil and forked mainnet nodes.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use alloy::primitives::{address, Address, Bytes, U256};
use alloy::providers::RootProvider;
use alloy::transports::http::reqwest::Url;
use alloy_dyn_abi::DynSolValue;
use serde_json::{json, Value};
use vaughan_core::browser::events::PairDiscovery;
use vaughan_core::browser::probe::ContractFingerprint;
use vaughan_core::browser::BrowserEngine;

struct Anvil {
    child: Child,
    port: u16,
}

impl Anvil {
    fn start() -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let child = Command::new("anvil")
            .args(["--port", &port.to_string(), "--chain-id", "943", "--silent"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("anvil must be on PATH (foundry)");
        let anvil = Self { child, port };

        // Wait for RPC readiness
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if anvil.rpc("eth_chainId", json!([])).is_ok() {
                return anvil;
            }
            if Instant::now() > deadline {
                panic!("anvil did not start within 10 seconds");
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
        Ok(v["result"].clone())
    }

    fn set_code(&self, addr: Address, code: &[u8]) {
        let hex_code = format!("0x{}", hex::encode(code));
        self.rpc("anvil_setCode", json!([format!("{addr:#x}"), hex_code]))
            .expect("anvil_setCode must succeed");
    }

    fn provider(&self) -> RootProvider {
        let url = Url::parse(&self.url()).unwrap();
        RootProvider::new_http(url)
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Assembles a valid EVM bytecode dispatcher that maps function selectors to static return data.
fn assemble_dispatcher(routes: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    // 1. Dispatch header:
    // PUSH1 0x00, CALLDATALOAD, PUSH1 0xe0, SHR
    let mut bytecode = vec![0x60, 0x00, 0x35, 0x60, 0xe0, 0x1c];

    // Compute jump table targets
    let dispatch_size = bytecode.len() + routes.len() * 11 + 5;
    let mut handlers = Vec::new();
    let mut current_offset = dispatch_size;

    for (_sel, ret_data) in routes {
        let handler_target = current_offset as u16;
        handlers.push((handler_target, ret_data.clone()));
        let chunks = if ret_data.is_empty() {
            0
        } else {
            (ret_data.len() + 31) / 32
        };
        current_offset += 1 + chunks * 36 + 6;
    }

    // Build dispatch jump table
    for (i, (sel, _)) in routes.iter().enumerate() {
        let (target, _) = handlers[i];
        bytecode.push(0x80); // DUP1
        bytecode.push(0x63); // PUSH4
        bytecode.extend_from_slice(sel);
        bytecode.push(0x14); // EQ
        bytecode.push(0x61); // PUSH2
        bytecode.push((target >> 8) as u8);
        bytecode.push((target & 0xff) as u8);
        bytecode.push(0x57); // JUMPI
    }

    // Fallback revert
    bytecode.extend_from_slice(&[0x60, 0x00, 0x60, 0x00, 0xfd]);

    // Build handlers
    for (target, ret_data) in handlers {
        assert_eq!(bytecode.len(), target as usize);
        bytecode.push(0x5b); // JUMPDEST
        let chunks = if ret_data.is_empty() {
            0
        } else {
            (ret_data.len() + 31) / 32
        };
        for c in 0..chunks {
            let start = c * 32;
            let end = (start + 32).min(ret_data.len());
            let mut chunk_bytes = [0u8; 32];
            chunk_bytes[..end - start].copy_from_slice(&ret_data[start..end]);
            bytecode.push(0x7f); // PUSH32
            bytecode.extend_from_slice(&chunk_bytes);
            bytecode.push(0x60); // PUSH1
            bytecode.push((c * 32) as u8); // mem offset
            bytecode.push(0x52); // MSTORE
        }
        let len = ret_data.len() as u16;
        bytecode.push(0x61); // PUSH2
        bytecode.push((len >> 8) as u8);
        bytecode.push((len & 0xff) as u8);
        bytecode.push(0x60); // PUSH1
        bytecode.push(0x00);
        bytecode.push(0xf3); // RETURN
    }

    bytecode
}

/// ABI-encode a string into ABI format (offset 0x20, length, data padded to 32 bytes).
fn abi_encode_string(s: &str) -> Vec<u8> {
    DynSolValue::String(s.to_string()).abi_encode()
}

/// ABI-encode a uint256.
fn abi_encode_u256(val: U256) -> Vec<u8> {
    DynSolValue::Uint(val, 256).abi_encode()
}

/// ABI-encode a uint8.
fn abi_encode_u8(val: u8) -> Vec<u8> {
    DynSolValue::Uint(U256::from(val), 8).abi_encode()
}

/// ABI-encode an address.
fn abi_encode_address(addr: Address) -> Vec<u8> {
    DynSolValue::Address(addr).abi_encode()
}

#[tokio::test]
async fn test_anvil_inspect_weth_and_dynamic_calls() {
    let anvil = Anvil::start();
    let provider = anvil.provider();
    let token_addr = address!("1111111111111111111111111111111111111111");

    // Build mock WETH bytecode:
    // name() -> "Wrapped Pulse"
    // symbol() -> "WPLS"
    // decimals() -> 18
    // totalSupply() -> 1,000,000 * 10^18
    // balanceOf(address) -> 500,000 * 10^18
    // deposit() -> empty
    // withdraw(uint256) -> empty
    let routes = vec![
        ([0x06, 0xfd, 0xde, 0x03], abi_encode_string("Wrapped Pulse")), // name()
        ([0x95, 0xd8, 0x9b, 0x41], abi_encode_string("WPLS")),          // symbol()
        ([0x31, 0x3c, 0xe7, 0xf2], abi_encode_u8(18)),                  // decimals()
        (
            [0x18, 0x16, 0x0d, 0xdd],
            abi_encode_u256(U256::from(1_000_000_000_000_000_000_000_000u128)),
        ), // totalSupply()
        (
            [0x70, 0xa0, 0x82, 0x31],
            abi_encode_u256(U256::from(500_000_000_000_000_000_000_000u128)),
        ), // balanceOf(address)
        ([0xd0, 0xe3, 0x0d, 0xb0], vec![]),                             // deposit()
        ([0x2e, 0x1a, 0x7d, 0x4d], vec![]),                             // withdraw(uint256)
    ];
    let bytecode = assemble_dispatcher(&routes);
    anvil.set_code(token_addr, &bytecode);

    let engine = BrowserEngine::new();
    let inspection = engine.inspect(&provider, 943, token_addr).await;

    // 1. Verify capability fingerprint
    assert_eq!(
        inspection.fingerprint,
        ContractFingerprint::Weth,
        "mock contract must be fingerprinted as Weth"
    );

    // 2. Verify PUSH4 candidate selector extraction
    assert!(inspection
        .candidate_selectors
        .contains(&[0x06, 0xfd, 0xde, 0x03]));
    assert!(inspection
        .candidate_selectors
        .contains(&[0x95, 0xd8, 0x9b, 0x41]));
    assert!(inspection
        .candidate_selectors
        .contains(&[0x31, 0x3c, 0xe7, 0xf2]));
    assert!(inspection
        .candidate_selectors
        .contains(&[0x18, 0x16, 0x0d, 0xdd]));
    assert!(inspection
        .candidate_selectors
        .contains(&[0x70, 0xa0, 0x82, 0x31]));
    assert!(inspection
        .candidate_selectors
        .contains(&[0xd0, 0xe3, 0x0d, 0xb0]));
    assert!(inspection
        .candidate_selectors
        .contains(&[0x2e, 0x1a, 0x7d, 0x4d]));

    // 3. Verify raw call execution
    let name_call = engine
        .call_raw(
            &provider,
            token_addr,
            Bytes::from(vec![0x06, 0xfd, 0xde, 0x03]),
        )
        .await
        .expect("name call");
    assert!(hex::encode(&name_call).contains(&hex::encode("Wrapped Pulse")));

    let total_supply_call = engine
        .call_raw(
            &provider,
            token_addr,
            Bytes::from(vec![0x18, 0x16, 0x0d, 0xdd]),
        )
        .await
        .expect("totalSupply call");
    assert_eq!(
        U256::from_be_slice(&total_supply_call),
        U256::from(1_000_000_000_000_000_000_000_000u128)
    );
}

#[tokio::test]
async fn test_anvil_inspect_uniswap_v2_factory_and_pair_discovery() {
    let anvil = Anvil::start();
    let provider = anvil.provider();
    let factory_addr = address!("2222222222222222222222222222222222222222");
    let pair_addr = address!("3333333333333333333333333333333333333333");
    let token0_addr = address!("4444444444444444444444444444444444444444");
    let token1_addr = address!("5555555555555555555555555555555555555555");

    // 1. Build Mock Factory:
    // allPairs(uint256) -> pair_addr
    // allPairsLength() -> 186,244
    // getPair(address,address) -> pair_addr
    let factory_routes = vec![
        ([0x1e, 0x3d, 0xd1, 0x8b], abi_encode_address(pair_addr)), // allPairs(uint256)
        (
            [0x57, 0x4f, 0x2b, 0xa3],
            abi_encode_u256(U256::from(186244)),
        ), // allPairsLength()
        ([0xe6, 0xa4, 0x39, 0x05], abi_encode_address(pair_addr)), // getPair(address,address)
    ];
    anvil.set_code(factory_addr, &assemble_dispatcher(&factory_routes));

    // 2. Build Mock Pair:
    // getReserves() -> (1000000, 2000000, 1700000000)
    // token0() -> token0_addr
    // token1() -> token1_addr
    let tuple_val = DynSolValue::Tuple(vec![
        DynSolValue::Uint(U256::from(1_000_000u64), 112),
        DynSolValue::Uint(U256::from(2_000_000u64), 112),
        DynSolValue::Uint(U256::from(1_700_000_000u64), 32),
    ]);
    let pair_routes = vec![
        ([0x09, 0x02, 0xf1, 0xac], tuple_val.abi_encode()), // getReserves()
        ([0x0f, 0xfe, 0xdf, 0xf8], abi_encode_address(token0_addr)), // token0()
        ([0xd2, 0x12, 0x20, 0xa7], abi_encode_address(token1_addr)), // token1()
    ];
    anvil.set_code(pair_addr, &assemble_dispatcher(&pair_routes));

    let engine = BrowserEngine::new();

    // Inspect Factory
    let factory_insp = engine.inspect(&provider, 943, factory_addr).await;
    assert!(
        matches!(
            factory_insp.fingerprint,
            ContractFingerprint::UniswapV2Factory {
                all_pairs_length: Some(186244)
            }
        ),
        "must detect UniswapV2Factory with pairs length"
    );

    // Test Pair Discovery count & range query
    let pair_count = PairDiscovery::get_v2_pairs_count(&provider, factory_addr)
        .await
        .expect("pair count");
    assert_eq!(pair_count, 186244);

    let pairs = PairDiscovery::fetch_v2_pairs_range(&provider, factory_addr, 0, 5).await;
    assert_eq!(pairs.len(), 5);
    for p in pairs {
        assert_eq!(p.pair_address, pair_addr);
    }

    // Inspect Pair
    let pair_insp = engine.inspect(&provider, 943, pair_addr).await;
    assert!(
        matches!(
            pair_insp.fingerprint,
            ContractFingerprint::UniswapV2Pair { .. }
        ),
        "must detect UniswapV2Pair"
    );

    // Call getReserves() on Pair
    let reserves_raw = engine
        .call_raw(
            &provider,
            pair_addr,
            Bytes::from(vec![0x09, 0x02, 0xf1, 0xac]),
        )
        .await
        .expect("getReserves call");
    assert_eq!(reserves_raw.len(), 96); // 3 * 32 bytes
}

#[tokio::test]
async fn test_anvil_inspect_uniswap_v3_pool() {
    let anvil = Anvil::start();
    let provider = anvil.provider();
    let pool_addr = address!("6666666666666666666666666666666666666666");
    let token0_addr = address!("4444444444444444444444444444444444444444");
    let token1_addr = address!("5555555555555555555555555555555555555555");

    // Build Mock Uniswap V3 Pool:
    // slot0() -> (sqrtPriceX96, tick, observationIndex, observationCardinality, observationCardinalityNext, feeProtocol, unlocked)
    // fee() -> 3000 (0.3%)
    // token0() -> token0_addr
    // token1() -> token1_addr
    let slot0_val = DynSolValue::Tuple(vec![
        DynSolValue::Uint(U256::from(79228162514264337593543950336u128), 160), // sqrtPriceX96 (1.0)
        DynSolValue::Int(alloy::primitives::I256::ZERO, 24),                   // tick 0
        DynSolValue::Uint(U256::from(1), 16),
        DynSolValue::Uint(U256::from(10), 16),
        DynSolValue::Uint(U256::from(10), 16),
        DynSolValue::Uint(U256::ZERO, 8),
        DynSolValue::Bool(true),
    ]);

    let pool_routes = vec![
        ([0x38, 0x50, 0xc7, 0xbd], slot0_val.abi_encode()), // slot0()
        ([0xdd, 0xca, 0x3f, 0x43], abi_encode_u256(U256::from(3000))), // fee()
        ([0x0f, 0xfe, 0xdf, 0xf8], abi_encode_address(token0_addr)), // token0()
        ([0xd2, 0x12, 0x20, 0xa7], abi_encode_address(token1_addr)), // token1()
    ];
    anvil.set_code(pool_addr, &assemble_dispatcher(&pool_routes));

    let engine = BrowserEngine::new();
    let pool_insp = engine.inspect(&provider, 943, pool_addr).await;

    assert!(
        matches!(
            pool_insp.fingerprint,
            ContractFingerprint::UniswapV3Pool { fee: 3000, .. }
        ),
        "must detect UniswapV3Pool with 3000 fee"
    );

    let slot0_raw = engine
        .call_raw(
            &provider,
            pool_addr,
            Bytes::from(vec![0x38, 0x50, 0xc7, 0xbd]),
        )
        .await
        .expect("slot0 call");
    assert_eq!(slot0_raw.len(), 7 * 32);
}

#[tokio::test]
async fn test_anvil_inspect_multicall3() {
    let anvil = Anvil::start();
    let provider = anvil.provider();
    let multicall_addr = address!("cA11bde05977b3631167028862bE2a173976CA11");

    // Mock Multicall3 tryAggregate (0xb1a3203d) returning an empty array
    let routes = vec![
        (
            [0xb1, 0xa3, 0x20, 0x3d],
            DynSolValue::Array(vec![]).abi_encode(),
        ), // tryAggregate(bool,Call[])
    ];
    anvil.set_code(multicall_addr, &assemble_dispatcher(&routes));

    let engine = BrowserEngine::new();
    let inspection = engine.inspect(&provider, 943, multicall_addr).await;

    assert_eq!(
        inspection.fingerprint,
        ContractFingerprint::Multicall3,
        "must detect Multicall3 fingerprint"
    );
}

#[tokio::test]
async fn test_anvil_inspect_generic_unverified_bytecode() {
    let anvil = Anvil::start();
    let provider = anvil.provider();
    let generic_addr = address!("9999999999999999999999999999999999999999");

    // Mock generic contract with arbitrary function selectors
    let routes = vec![
        ([0xaa, 0xbb, 0xcc, 0xdd], abi_encode_u256(U256::from(42))),
        ([0xee, 0xff, 0x11, 0x22], abi_encode_u256(U256::from(100))),
    ];
    anvil.set_code(generic_addr, &assemble_dispatcher(&routes));

    let engine = BrowserEngine::new();
    let inspection = engine.inspect(&provider, 943, generic_addr).await;

    // Must detect Generic with has_code: true
    assert!(
        matches!(
            inspection.fingerprint,
            ContractFingerprint::Generic { has_code: true, .. }
        ),
        "must detect Generic contract"
    );

    // Must extract candidate selectors accurately
    assert!(inspection
        .candidate_selectors
        .contains(&[0xaa, 0xbb, 0xcc, 0xdd]));
    assert!(inspection
        .candidate_selectors
        .contains(&[0xee, 0xff, 0x11, 0x22]));

    // Executing raw call on candidate selector must succeed and return 42
    let res = engine
        .call_raw(
            &provider,
            generic_addr,
            Bytes::from(vec![0xaa, 0xbb, 0xcc, 0xdd]),
        )
        .await
        .expect("raw call");
    assert_eq!(U256::from_be_slice(&res), U256::from(42));
}
