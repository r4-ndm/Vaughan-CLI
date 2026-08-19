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

use vaughan_core::chains::evm::tokens;
use vaughan_core::chains::evm::EvmAdapter;

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
        .args([
            "--fork-url",
            FORK_RPC,
            "--port",
            &port.to_string(),
            "--silent",
        ])
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
    assert!(
        symbol.contains('…'),
        "unknown token shows a shortened address: {symbol}"
    );
}

/// Same scenario, but with Multicall3 **absent** (its code wiped on the fork):
/// `get_assets` must fall back to sequential `balanceOf` reads and still
/// return the exact same asset set. This is the path a chain *without*
/// Multicall3 takes, and it must not silently lose tokens.
#[tokio::test]
#[ignore = "requires anvil (foundry) + network fork of PulseChain mainnet"]
async fn assets_detect_without_multicall3_sequential_fallback() {
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

    // ---- wipe Multicall3's code so the probe sees an empty address ----
    wp.client()
        .request::<_, ()>(
            "anvil_setCode",
            (vaughan_core::chains::evm::adapter::MULTICALL3, "0x"),
        )
        .await
        .expect("anvil_setCode");

    // ---- wrap 1 PLS → WPLS (same as the batch test) ----
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
                .value(U256::from(10u128.pow(18)))
                .input(vec![0xd0, 0xe3, 0x0d, 0xb0].into()),
        )
        .await
        .expect("wrap broadcast");
    let receipt = pending.get_receipt().await.expect("wrap receipt");
    assert!(receipt.status(), "WPLS wrap reverted");

    // ---- get_assets via the sequential fallback: same result set ----
    let assets = adapter
        .get_assets(&me.to_string())
        .await
        .expect("get_assets without Multicall3");

    let native = assets
        .iter()
        .find(|b| b.token.contract_address.is_none())
        .expect("native PLS balance present");
    assert_eq!(native.token.symbol, "PLS");
    assert_ne!(native.raw, "0", "dev account is funded on the fork");

    let wpls_bal = assets
        .iter()
        .find(|b| b.token.symbol == "WPLS")
        .expect("WPLS detected through the sequential fallback");
    assert_eq!(wpls_bal.token.name, "Wrapped Pulse");
    assert!(
        wpls_bal.formatted.starts_with('1'),
        "wrapped exactly 1 PLS, got {}",
        wpls_bal.formatted
    );

    // Zero-balance curated tokens still excluded on this path.
    assert!(
        assets.iter().all(|b| b.token.symbol != "HEX"),
        "zero-balance tokens must not appear in the asset list"
    );
}

/// Auto asset discovery: a **non-curated** token the wallet receives must
/// appear in `get_assets` purely from its EIP-20 `Transfer` log — no registry
/// entry needed.
///
/// Uses anvil's impersonation to move LINK from a real whale on the
/// PulseChain fork (`anvil_impersonateAccount`), then asserts `get_assets`
/// lists LINK with on-chain metadata.
#[tokio::test]
#[ignore = "requires anvil (foundry) + network fork of PulseChain mainnet"]
async fn assets_detect_transfer_discovered_token() {
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

    // ---- LINK: real token on the fork, NOT in the curated registry ----
    // Verified 2026-08-18: symbol LINK, 18 decimals (see docs/optimizations.md).
    let link: Address = "0x514910771AF9Ca656af840dff83E8264EcF986CA"
        .parse()
        .unwrap();
    // A known LINK whale on PulseChain (top holder, 2026-08-18 explorer data).
    let whale: Address = "0xF977814e90dA44bFA03b6295A0616a897441aceC"
        .parse()
        .unwrap();

    // Impersonate the whale on the fork: anvil signs for it and we can move
    // its LINK. Fund it for gas first.
    wp.client()
        .request::<_, ()>("anvil_impersonateAccount", (whale,))
        .await
        .expect("impersonate");
    wp.client()
        .request::<_, ()>("anvil_setBalance", (whale, U256::from(10u128.pow(20))))
        .await
        .expect("fund whale");

    // Whale transfers 5 LINK to the dev wallet: transfer(address,uint256).
    // Sent through a *walletless* provider: anvil impersonation executes
    // unsigned `eth_sendTransaction` from the impersonated account (a signer
    // wallet would try to sign as the whale and fail).
    let anon = ProviderBuilder::new().connect_http(anvil.url.parse().unwrap());
    let amount = U256::from(5u128) * U256::from(10u128.pow(18));
    let mut calldata = vec![0xa9, 0x05, 0x9c, 0xbb]; // transfer(address,uint256)
    calldata.extend_from_slice(&[0u8; 12]);
    calldata.extend_from_slice(me.as_slice());
    calldata.extend_from_slice(&amount.to_be_bytes::<32>());
    let pending = anon
        .send_transaction(
            TransactionRequest::default()
                .from(whale)
                .to(link)
                .input(calldata.into()),
        )
        .await
        .expect("LINK transfer broadcast");
    let receipt = pending.get_receipt().await.expect("LINK transfer receipt");
    assert!(receipt.status(), "LINK transfer reverted");

    // ---- get_assets must now show LINK (discovered from the Transfer log) ----
    let assets = adapter
        .get_assets(&me.to_string())
        .await
        .expect("get_assets with discovered token");
    let link_bal = assets
        .iter()
        .find(|b| b.token.symbol == "LINK")
        .expect("LINK must appear via Transfer-event discovery");
    assert_eq!(link_bal.token.decimals, 18);
    assert!(
        link_bal
            .token
            .contract_address
            .as_deref()
            .is_some_and(|a| a.eq_ignore_ascii_case("0x514910771AF9Ca656af840dff83E8264EcF986CA")),
        "LINK address mismatch"
    );
    assert!(
        link_bal.formatted.starts_with('5'),
        "received 5 LINK, got {}",
        link_bal.formatted
    );

    // The curated tokens are still there too (native balance unchanged).
    assert!(
        assets.iter().any(|b| b.token.contract_address.is_none()),
        "native balance must still be present"
    );
}
