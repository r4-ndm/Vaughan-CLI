//! Anvil integration test: deploy fixed-supply ERC-20 via `token_launch`.

mod common;

use alloy::primitives::Address;
use std::str::FromStr;
use tokio::runtime::Runtime;
use vaughan_core::core::TOKEN_LAUNCH_DECIMALS;

#[test]
fn anvil_deploy_fixed_supply_meme_token() {
    let anvil = common::Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = common::funded_wallet(dir.path(), &anvil);
    let owner = wallet.active_address().unwrap().to_string();

    let rt = Runtime::new().expect("tokio runtime");
    let outcome = rt
        .block_on(wallet.deploy_fixed_supply_token("Test Meme", "meme", "1000000"))
        .expect("deploy_fixed_supply_token");

    assert_eq!(outcome.token.symbol, "MEME");
    assert_eq!(outcome.token.name, "Test Meme");
    assert_eq!(outcome.token.decimals, TOKEN_LAUNCH_DECIMALS);
    assert!(outcome.tx_hash.starts_with("0x"));

    let code = anvil
        .rpc(
            "eth_getCode",
            serde_json::json!([outcome.token.address, "latest"]),
        )
        .expect("getCode");
    assert_ne!(code.as_str().unwrap(), "0x");

    let owner_addr = Address::from_str(&owner).unwrap();
    let token = Address::from_str(&outcome.token.address).unwrap();
    let balance_of = {
        use alloy::sol_types::SolCall;
        alloy::sol! {
            function balanceOf(address account) external view returns (uint256);
        }
        let data = balanceOfCall {
            account: owner_addr,
        }
        .abi_encode();
        let raw = anvil
            .rpc(
                "eth_call",
                serde_json::json!([{
                    "to": format!("{token:#x}"),
                    "data": format!("0x{}", hex::encode(data)),
                }, "latest"]),
            )
            .expect("balanceOf");
        u256_hex_to_u128(raw.as_str().unwrap())
    };
    assert_eq!(balance_of, 1_000_000u128 * 10u128.pow(18));

    let imported = wallet
        .custom_tokens_for_active_chain()
        .into_iter()
        .any(|t| t.address.eq_ignore_ascii_case(&outcome.token.address));
    assert!(imported, "token should be auto-imported");
}

fn u256_hex_to_u128(s: &str) -> u128 {
    let s = s.trim_start_matches("0x");
    if s.is_empty() {
        return 0;
    }
    let bytes = hex::decode(s).expect("hex");
    let mut buf = [0u8; 32];
    let start = 32usize.saturating_sub(bytes.len());
    buf[start..].copy_from_slice(&bytes);
    u128::from_be_bytes(buf[16..32].try_into().unwrap())
}
