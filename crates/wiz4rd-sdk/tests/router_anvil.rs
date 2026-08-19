//! End-to-end Anvil integration test for wiz4rd-sdk V3 SwapRouter calldata execution.

use alloy::eips::eip2718::Encodable2718;
use alloy::network::{EthereumWallet, NetworkTransactionBuilder, TransactionBuilder};
use alloy::primitives::{address, Address, U256};
use alloy::providers::Provider;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolCall;
use serde_json::json;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use url::Url;

use wiz4rd_sdk::config::Config;
use wiz4rd_sdk::pool::PoolInfo;
use wiz4rd_sdk::pool_address::PoolKey;
use wiz4rd_sdk::tx::swap::build_swap_exact_in;

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
            .args(["--port", &port.to_string(), "--chain-id", "31337", "--silent"])
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
                panic!("anvil did not start within 10 seconds");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn rpc(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
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
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
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
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
            (ret_data.len() + 31) / 32
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
            (ret_data.len() + 31) / 32
        };
        for c in 0..chunks {
            let start = c * 32;
            let end = (start + 32).min(ret_data.len());
            let mut chunk_bytes = [0u8; 32];
            chunk_bytes[..end - start].copy_from_slice(&ret_data[start..end]);
            bytecode.push(0x7f);
            bytecode.extend_from_slice(&chunk_bytes);
            bytecode.push(0x60);
            bytecode.push((c * 32) as u8);
            bytecode.push(0x52);
        }
        let len = ret_data.len() as u16;
        bytecode.push(0x61);
        bytecode.push((len >> 8) as u8);
        bytecode.push((len & 0xff) as u8);
        bytecode.push(0x60);
        bytecode.push(0x00);
        bytecode.push(0xf3);
    }

    bytecode
}

#[tokio::test]
async fn test_anvil_v3_swap_router_execution() {
    let anvil = Anvil::start();
    let router_addr = address!("1111111111111111111111111111111111111111");
    let pool_addr = address!("2222222222222222222222222222222222222222");
    let token_in = address!("3333333333333333333333333333333333333333");
    let token_out = address!("4444444444444444444444444444444444444444");

    // exactInputSingle selector = 0x04e45aaf (or ISwapRouter::exactInputSingleCall::SELECTOR)
    // Return amountOut = 950 * 10^18
    let amount_out = U256::from(950) * U256::from(10).pow(U256::from(18));
    let mut out_bytes = vec![0u8; 32];
    out_bytes.copy_from_slice(&amount_out.to_be_bytes::<32>());

    let exact_input_single_sel = wiz4rd_sdk::abi::ISwapRouter::exactInputSingleCall::SELECTOR;
    let routes = vec![(exact_input_single_sel, out_bytes)];
    anvil.set_code(router_addr, &assemble_dispatcher(&routes));

    // Setup caller wallet on Anvil (Account 0)
    let signer: PrivateKeySigner = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
        .parse()
        .unwrap();
    let sender_addr = signer.address();
    let wallet = EthereumWallet::new(signer);
    let rpc_url: Url = anvil.url().parse().unwrap();
    let provider = alloy::providers::RootProvider::<alloy::network::Ethereum>::new_http(rpc_url);

    let config = Config {
        swap_router: Some(router_addr),
        ..Config::default()
    };

    let pool_info = PoolInfo {
        pool_key: PoolKey {
            token0: token_in,
            token1: token_out,
            fee: 500,
        },
        pool: pool_addr,
        token0: token_in,
        token1: token_out,
        fee: 500,
        sqrt_price_x96: U256::from(1) << 96,
        tick: 0,
        fee_protocol: 0,
        liquidity: 10_000_000,
    };

    // Build swap transaction calldata via wiz4rd-sdk
    let mut tx_req = build_swap_exact_in(
        &config,
        &pool_info,
        token_in,
        U256::from(100) * U256::from(10).pow(U256::from(18)),
        U256::from(900) * U256::from(10).pow(U256::from(18)),
        sender_addr,
        1_800_000_000,
        None,
    )
    .unwrap();

    let nonce = provider.get_transaction_count(sender_addr).await.unwrap();
    tx_req.set_nonce(nonce);
    tx_req.set_chain_id(31337);
    tx_req.set_gas_limit(100_000);
    tx_req.set_max_fee_per_gas(20_000_000_000);
    tx_req.set_max_priority_fee_per_gas(1_000_000_000);

    // Sign and broadcast to Anvil
    let signed = tx_req.build(&wallet).await.unwrap();
    let pending = provider
        .send_raw_transaction(&signed.encoded_2718())
        .await
        .unwrap();

    let receipt = pending.get_receipt().await.unwrap();
    assert_eq!(
        receipt.status(),
        true,
        "wiz4rd-sdk swap transaction must succeed on Anvil"
    );
}
