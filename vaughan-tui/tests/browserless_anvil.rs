//! Anvil tests for browserless Pulse primitives: WPLS wrap/unwrap + ERC-20 revoke.
//!
//! Plants [`fixtures/mock_weth.runtime.hex`] (real WETH9-shaped storage), then
//! broadcasts txs built by [`vaughan_tui::views::dex_calldata`] — the same
//! helpers a future Wrap / Approvals TUI will use.
//!
//! ```sh
//! cargo test -p vaughan-tui --test browserless_anvil -- --nocapture
//! ```

mod common;

use alloy::primitives::{address, Address, U256};
use common::{funded_wallet, Anvil};
use serde_json::json;
use std::str::FromStr;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use vaughan_tui::views::dex_calldata::{
    build_approve_tx, build_revoke_tx, build_unwrap_tx, build_wrap_tx, encode_allowance_call,
    encode_balance_of_call, weth_deposit_selector, weth_withdraw_selector,
};

const MOCK_WPLS: Address = address!("0xA1077a294dDE1B09bB078844df40758a5D0f9a27");
const SPENDER: Address = address!("0xDa8953Fc615d6E816b9647Afd5536123dcE70B78");

fn mock_weth_runtime() -> Vec<u8> {
    let hex = include_str!("fixtures/mock_weth.runtime.hex")
        .trim()
        .trim_start_matches("0x");
    hex::decode(hex).expect("mock_weth.runtime.hex")
}

fn plant_wpls(anvil: &Anvil) {
    anvil
        .rpc(
            "anvil_setCode",
            json!([
                format!("{MOCK_WPLS:#x}"),
                format!("0x{}", hex::encode(mock_weth_runtime()))
            ]),
        )
        .expect("anvil_setCode mock wpls");
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

fn eth_call_u256(anvil: &Anvil, to: Address, data: &str) -> U256 {
    let result = anvil
        .rpc(
            "eth_call",
            json!([{ "to": format!("{to:#x}"), "data": data }, "latest"]),
        )
        .expect("eth_call");
    let hex = result.as_str().expect("eth_call hex");
    U256::from_str(hex)
        .unwrap_or_else(|_| U256::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap())
}

#[test]
fn calldata_selectors_match_weth9() {
    assert_eq!(weth_deposit_selector(), [0xd0, 0xe3, 0x0d, 0xb0]);
    assert_eq!(weth_withdraw_selector(), [0x2e, 0x1a, 0x7d, 0x4d]);
}

#[test]
fn anvil_wpls_wrap_then_unwrap() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let rt = Runtime::new().unwrap();
    plant_wpls(&anvil);

    let from = wallet.active_address().unwrap().to_string();
    let owner: Address = from.parse().unwrap();
    let amount = U256::from(10u64).pow(U256::from(16)); // 0.01

    let native_before = anvil.wei_balance(&from);

    let wrap = build_wrap_tx(MOCK_WPLS, amount, &from, 943);
    assert_eq!(wrap.value, amount.to_string());
    let wh = rt
        .block_on(wallet.send_transaction(wrap))
        .unwrap_or_else(|e| panic!("wrap failed: {}", e.user_message()));
    wait_receipt(&anvil, &wh.to_string());

    let bal = eth_call_u256(&anvil, MOCK_WPLS, &encode_balance_of_call(owner));
    assert_eq!(bal, amount, "WPLS balance after deposit");

    let native_mid = anvil.wei_balance(&from);
    assert!(
        native_mid < native_before,
        "native should drop after wrap (before={native_before} mid={native_mid})"
    );

    let unwrap = build_unwrap_tx(MOCK_WPLS, amount, &from, 943);
    let uh = rt
        .block_on(wallet.send_transaction(unwrap))
        .unwrap_or_else(|e| panic!("unwrap failed: {}", e.user_message()));
    wait_receipt(&anvil, &uh.to_string());

    let bal_after = eth_call_u256(&anvil, MOCK_WPLS, &encode_balance_of_call(owner));
    assert_eq!(bal_after, U256::ZERO, "WPLS balance after withdraw");

    let native_after = anvil.wei_balance(&from);
    assert!(
        native_after > native_mid,
        "native should rise after unwrap (mid={native_mid} after={native_after})"
    );
}

#[test]
fn anvil_erc20_approve_then_revoke_clears_allowance() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let rt = Runtime::new().unwrap();
    plant_wpls(&anvil);

    let from = wallet.active_address().unwrap().to_string();
    let owner: Address = from.parse().unwrap();
    let amount = U256::from(10u64).pow(U256::from(18));

    // Need a token balance only if transfer — approve does not.
    let approve = build_approve_tx(MOCK_WPLS, SPENDER, amount, &from, 943);
    let ah = rt
        .block_on(wallet.send_transaction(approve))
        .unwrap_or_else(|e| panic!("approve failed: {}", e.user_message()));
    wait_receipt(&anvil, &ah.to_string());

    let allowed = eth_call_u256(&anvil, MOCK_WPLS, &encode_allowance_call(owner, SPENDER));
    assert_eq!(allowed, amount, "allowance after approve");

    let revoke = build_revoke_tx(MOCK_WPLS, SPENDER, &from, 943);
    let rh = rt
        .block_on(wallet.send_transaction(revoke))
        .unwrap_or_else(|e| panic!("revoke failed: {}", e.user_message()));
    wait_receipt(&anvil, &rh.to_string());

    let cleared = eth_call_u256(&anvil, MOCK_WPLS, &encode_allowance_call(owner, SPENDER));
    assert_eq!(cleared, U256::ZERO, "allowance after revoke");
}

/// Browserless EIP-712 (no dApp): `execute_approval_sync` on `SignTypedData`
/// must match foundry's reference signer — same path as browser `sign-typed` → Approve.
#[test]
fn anvil_browserless_sign_typed_data_matches_foundry() {
    use std::process::Command;
    use vaughan_tui::provider::{execute_approval_sync, ApprovalKind};

    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let rt = Runtime::new().unwrap();
    let handle = rt.handle().clone();

    let sender = wallet.active_address().unwrap().to_string();
    let typed_data = json!({
        "types": {
            "EIP712Domain": [
                {"name": "name", "type": "string"},
                {"name": "version", "type": "string"},
                {"name": "chainId", "type": "uint256"},
            ],
            "Message": [{"name": "content", "type": "string"}],
        },
        "primaryType": "Message",
        "domain": {"name": "Vaughan Browserless", "version": "1", "chainId": 943},
        "message": {"content": "Hello, browserless Pulse!"},
    });

    let kind = ApprovalKind::SignTypedData {
        address: sender.clone(),
        typed_data: typed_data.clone(),
    };
    let signature = execute_approval_sync(&kind, &wallet, &handle)
        .unwrap_or_else(|e| panic!("browserless sign typed data failed: {e}"));
    assert!(signature.starts_with("0x"));
    assert_eq!(signature.len(), 2 + 65 * 2);

    let out = Command::new("cast")
        .args([
            "wallet",
            "sign",
            "--data",
            "--private-key",
            common::ANVIL_KEY0,
        ])
        .arg(typed_data.to_string())
        .output()
        .expect("cast must be available");
    assert!(
        out.status.success(),
        "cast wallet sign failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reference = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(
        signature, reference,
        "browserless EIP-712 must match foundry reference"
    );
}
