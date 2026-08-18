//! E2E for **auto asset detection** against an [`anvil`](https://book.getfoundry.sh/anvil/)
//! fork of **PulseChain mainnet** (chain 369) — the real target chain, with
//! the real Multicall3 and real EIP-20 contracts:
//!
//! 1. wrap PLS → WPLS for the funded dev account (real WPLS `deposit()`);
//! 2. `get_assets` must return the native PLS balance **and** the WPLS
//!    balance (read through the one Multicall3 `tryAggregate` batch), with
//!    on-chain symbol/decimals;
//! 3. zero-balance curated tokens (e.g. HEX) are excluded from the batch;
//! 4. a single-token read for a zero balance still carries correct metadata;
//! 5. an unknown (registry-less) address falls back to a shortened address +
//!    18 decimals for metadata.
//!
//! Provenance for each piece: `docs/optimizations.md`.
//!
//! Run with:
//! ```sh
//! cargo test -p vaughan-core --test assets_e2e -- --ignored --nocapture
//! ```
//! Requires the `anvil` binary (foundry) and network access to
//! rpc.pulsechain.com.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;

use vaughan_core::chains::evm::EvmAdapter;
use vaughan_core::chains::evm::tokens;

/// anvil's first dev account: pre-funded on forks, deterministic key.
const ANVIL_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const FORK_RPC: &str = "https://rpc.pulsechain.com";

struct Anvil {
    child: Child,
    url: String,
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_anvil() -> Anvil {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}");
    let child = Command::new("anvil")
        .args(["--fork-url", FORK_RPC, "--port", &port.to_string(), "--silent"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn anvil — install foundry (https://book.getfoundry.sh)");
    Anvil { child, url }
}

#[tokio::test]
#[ignore = "requires anvil (foundry) + network fork of PulseChain mainnet"]
async fn assets_detect_native_and_erc20_on_pulsechain_fork() {
    let anvil = spawn_anvil();
    let adapter = EvmAdapter::new(&anvil.url, 369, "PulseChain Mainnet", &[])
        .await
        .expect("adapter");

    let signer: PrivateKeySigner = ANVIL_KEY.parse().expect("anvil key");
    let me = signer.address();
    let wp = ProviderBuilder::new()
        .wallet(EthereumWallet::from(signer))
        .connect_http(anvil.url.parse().unwrap());

    let mut ready = false;
    for _ in 0..60 {
        if wp.get_chain_id().await.is_ok() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(ready, "anvil did not come up on {}", anvil.url);

    // ---- wrap 1 PLS → WPLS through the real WPLS contract on the fork ----
    let mainnet = tokens::pulsechain_mainnet_tokens();
    let wpls: Address = mainnet
        .iter()
        .find(|t| t.symbol == "WPLS")
        .expect("registry WPLS")
        .address
        .parse()
        .unwrap();
    let pending = wp
        .send_transaction(
            TransactionRequest::default()
                .to(wpls)
                .value(U256::from(10u128.pow(18))) // 1 PLS
                .input(vec![0xd0, 0xe3, 0x0d, 0xb0].into()), // WPLS.deposit()
        )
        .await
        .expect("wrap broadcast");
    let receipt = pending.get_receipt().await.expect("wrap receipt");
    assert!(receipt.status(), "WPLS wrap reverted");

    // ---- get_assets: native + WPLS, zero tokens excluded ----
    let assets = adapter
        .get_assets(&me.to_string())
        .await
        .expect("get_assets");

    let native = assets
        .iter()
        .find(|b| b.token.contract_address.is_none())
        .expect("native PLS balance present");
    assert_eq!(native.token.symbol, "PLS");
    assert_eq!(native.token.decimals, 18);
    assert_ne!(native.raw, "0", "dev account is funded on the fork");

    let wpls_bal = assets
        .iter()
        .find(|b| b.token.symbol == "WPLS")
        .expect("WPLS detected through the multicall batch");
    assert_eq!(wpls_bal.token.decimals, 18);
    assert_eq!(wpls_bal.token.name, "Wrapped Pulse");
    assert!(
        wpls_bal.formatted.starts_with('1'),
        "wrapped exactly 1 PLS, got {}",
        wpls_bal.formatted
    );

    // Zero-balance curated tokens (HEX, PLSX, INC, USDT, USDC) are excluded.
    assert!(
        assets.iter().all(|b| b.token.symbol != "HEX"),
        "zero-balance tokens must not appear in the asset list"
    );

    // ---- single-token read: zero balance still carries correct metadata ----
    let mainnet = tokens::pulsechain_mainnet_tokens();
    let hex = mainnet.iter().find(|t| t.symbol == "HEX").unwrap();
    let hex_bal = adapter
        .get_token_balance(hex.address, &me.to_string())
        .await
        .expect("HEX balance");
    assert_eq!(hex_bal.token.symbol, "HEX");
    assert_eq!(hex_bal.token.decimals, 8);
    assert_eq!(hex_bal.raw, "0");

    // ---- unknown address: metadata falls back, never errors ----
    let (symbol, _name, decimals) = adapter
        .get_token_metadata("0x1111111111111111111111111111111111111111")
        .await
        .expect("metadata fallback");
    assert_eq!(decimals, 18, "unknown token defaults to 18 decimals");
    assert!(symbol.contains('…'), "unknown token shows a shortened address: {symbol}");
}
