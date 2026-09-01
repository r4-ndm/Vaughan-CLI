//! Deterministic sensory tool tests against a local Anvil node.

use alloy::primitives::address;
use alloy::providers::Provider;
use serde_json::json;
use std::process::{Child, Command};
use std::time::Duration;
use url::Url;

use vaughan_agent::tools::{default_sensory_registry, ToolContext};

struct AnvilGuard {
    child: Child,
    rpc_url: String,
}

impl AnvilGuard {
    fn spawn(port: u16) -> Self {
        let child = Command::new("anvil")
            .args(["-p", &port.to_string(), "--silent"])
            .spawn()
            .expect("Failed to start Anvil. Make sure Foundry/Anvil is installed.");

        std::thread::sleep(Duration::from_millis(400));
        let rpc_url = format!("http://127.0.0.1:{}", port);
        Self { child, rpc_url }
    }
}

impl Drop for AnvilGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[tokio::test]
async fn test_tool_registry_and_sensory_tools_with_anvil() {
    let anvil = AnvilGuard::spawn(8555);
    let registry = default_sensory_registry();
    let defs = registry.definitions();

    assert!(
        defs.len() >= 14,
        "sensory registry shrank unexpectedly: {}",
        defs.len()
    );
    let names: Vec<String> = defs.into_iter().map(|d| d.name).collect();
    assert!(names.contains(&"inspect_contract".to_string()));
    assert!(names.contains(&"get_balance".to_string()));
    assert!(names.contains(&"get_dex_reserves".to_string()));
    assert!(names.contains(&"search_pairs".to_string()));
    assert!(names.contains(&"simulate_call".to_string()));
    assert!(names.contains(&"quote_bridge".to_string()));
    assert!(names.contains(&"list_transfers".to_string()));
    assert!(names.contains(&"watch_balance".to_string()));
    assert!(names.contains(&"watch_quote".to_string()));

    let context = ToolContext {
        rpc_url: anvil.rpc_url.clone(),
        chain_id: 31337,
        active_address: Some(address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266")),
            profile_dir: None,
    };

    // 1. Test get_balance tool
    let bal_res = registry
        .execute(
            "get_balance",
            json!({
                "account_address": "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
            }),
            &context,
        )
        .await
        .unwrap();

    assert_eq!(
        bal_res["account"].as_str().unwrap().to_ascii_lowercase(),
        "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
    );
    assert!(bal_res["balance_wei"]
        .as_str()
        .unwrap()
        .starts_with("10000"));

    // 2. Test simulate_call tool (native transfer dry run)
    let sim_res = registry
        .execute(
            "simulate_call",
            json!({
                "to": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
                "data": "0x",
                "value": "1000000000000000000"
            }),
            &context,
        )
        .await
        .unwrap();

    assert_eq!(sim_res["status"], "success");
    assert_eq!(sim_res["reverted"], false);

    // 3. Test deploy WETH bytecode via anvil_setCode and test inspect_contract tool
    let provider: alloy::providers::RootProvider<alloy::network::Ethereum> =
        alloy::providers::RootProvider::new_http(Url::parse(&anvil.rpc_url).unwrap());

    let weth_addr = address!("1111111111111111111111111111111111111111");

    // name() [0x06fdde03], symbol() [0x95d89b41], decimals() [0x313ce7f2], totalSupply() [0x18160ddd], deposit() [0xd0e30db0], withdraw() [0x2e1a7d4d]
    let routes: &[([u8; 4], Vec<u8>)] = &[
        ([0x06, 0xfd, 0xde, 0x03], vec![0; 64]), // name()
        ([0x95, 0xd8, 0x9b, 0x41], vec![0; 64]), // symbol()
        ([0x31, 0x3c, 0xe7, 0xf2], vec![0; 32]), // decimals()
        ([0x18, 0x16, 0x0d, 0xdd], vec![0; 32]), // totalSupply()
        ([0xd0, 0xe3, 0x0d, 0xb0], vec![]),      // deposit()
        ([0x2e, 0x1a, 0x7d, 0x4d], vec![]),      // withdraw(uint256)
    ];

    let bytecode = assemble_dispatcher(routes);
    let _: () = provider
        .raw_request(
            "anvil_setCode".into(),
            (weth_addr, format!("0x{}", hex::encode(bytecode))),
        )
        .await
        .unwrap();

    let inspect_res = registry
        .execute(
            "inspect_contract",
            json!({
                "address": "0x1111111111111111111111111111111111111111"
            }),
            &context,
        )
        .await
        .unwrap();

    assert_eq!(inspect_res["fingerprint"]["type"], "Weth");
    assert!(inspect_res["candidate_selectors"].as_array().unwrap().len() >= 6);
}

#[tokio::test]
async fn test_get_dex_reserves_spot_price_on_anvil() {
    let anvil = AnvilGuard::spawn(8556);
    let registry = default_sensory_registry();
    let pair_addr = address!("2222222222222222222222222222222222222222");
    let token0 = address!("3333333333333333333333333333333333333333");
    let token1 = address!("4444444444444444444444444444444444444444");

    // reserve0 = 100 * 10^18, reserve1 = 200 * 10^18 -> spot price = 2.0
    let mut reserves_bytes = vec![0u8; 96];
    let r0 = alloy::primitives::U256::from(100)
        * alloy::primitives::U256::from(10).pow(alloy::primitives::U256::from(18));
    let r1 = alloy::primitives::U256::from(200)
        * alloy::primitives::U256::from(10).pow(alloy::primitives::U256::from(18));
    reserves_bytes[..32].copy_from_slice(&r0.to_be_bytes::<32>());
    reserves_bytes[32..64].copy_from_slice(&r1.to_be_bytes::<32>());
    reserves_bytes[64..96]
        .copy_from_slice(&alloy::primitives::U256::from(1700000000).to_be_bytes::<32>());

    let mut t0_bytes = vec![0u8; 32];
    t0_bytes[12..32].copy_from_slice(token0.as_slice());

    let mut t1_bytes = vec![0u8; 32];
    t1_bytes[12..32].copy_from_slice(token1.as_slice());

    let routes = vec![
        ([0x0d, 0xfe, 0x16, 0x81], t0_bytes),       // token0()
        ([0xd2, 0x12, 0x20, 0xa7], t1_bytes),       // token1()
        ([0x09, 0x02, 0xf1, 0xac], reserves_bytes), // getReserves()
    ];

    let code_hex = format!("0x{}", hex::encode(assemble_dispatcher(&routes)));
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "anvil_setCode",
        "params": [format!("{pair_addr:#x}"), code_hex]
    });

    let _ = std::process::Command::new("curl")
        .args(["-s", "-X", "POST", "-H", "Content-Type: application/json"])
        .arg("-d")
        .arg(body.to_string())
        .arg(&anvil.rpc_url)
        .output()
        .unwrap();

    let context = ToolContext {
        rpc_url: anvil.rpc_url.clone(),
        chain_id: 31337,
        active_address: Some(address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266")),
            profile_dir: None,
    };

    let res = registry
        .execute(
            "get_dex_reserves",
            json!({
                "pair_address": format!("{pair_addr:#x}")
            }),
            &context,
        )
        .await
        .unwrap();

    assert_eq!(res["token0"], format!("{token0:#x}"));
    assert_eq!(res["token1"], format!("{token1:#x}"));
    assert_eq!(res["spot_price_token1_per_token0"], 2.0);
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

    for (target, ret_data) in handlers {
        assert_eq!(bytecode.len(), target as usize);
        bytecode.push(0x5b); // JUMPDEST
        let chunks = if ret_data.is_empty() {
            0
        } else {
            ret_data.len().div_ceil(32)
        };
        for c in 0..chunks {
            let start = c * 32;
            let end = (start + 32).min(ret_data.len());
            let mut chunk_bytes = [0u8; 32];
            chunk_bytes[..end - start].copy_from_slice(&ret_data[start..end]);
            bytecode.push(0x7f); // PUSH32
            bytecode.extend_from_slice(&chunk_bytes);
            bytecode.push(0x60); // PUSH1
            bytecode.push((c * 32) as u8);
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
