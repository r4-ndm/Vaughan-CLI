//! Live wiz4rd tools against PulseChain testnet 943 (needs network).

use alloy::primitives::address;
use serde_json::json;
use vaughan_agent::tools::{default_assist_registry, ToolContext};

#[tokio::test]
#[ignore = "hits rpc.v4.testnet.pulsechain.com — run with --ignored when online"]
async fn wiz4rd_get_pool_and_quote_on_943() {
    let registry = default_assist_registry();
    let ctx = ToolContext {
        rpc_url: "https://rpc.v4.testnet.pulsechain.com".into(),
        chain_id: 943,
        active_address: Some(address!("0xAe089fF30590206F24E4E6627Ea61E4944cFc895")),
    };

    let pool = registry
        .execute(
            "get_v3_pool",
            json!({
                "token_a": "WPLS",
                "token_b": "WZRD",
                "fee": 500
            }),
            &ctx,
        )
        .await
        .expect("get_v3_pool");
    assert_eq!(
        pool["pool"].as_str().unwrap().to_lowercase(),
        "0xd47e01c1af55a48c11d0e324fb1853cf2501e0dc"
    );

    let quote = registry
        .execute(
            "quote_v3_swap",
            json!({
                "token_in": "WZRD",
                "token_out": "WPLS",
                "amount_in": "10000000000000000",
                "fee": 500
            }),
            &ctx,
        )
        .await
        .expect("quote_v3_swap");
    assert!(quote["amount_out"].as_str().is_some());
}

#[test]
fn wiz4rd_tools_registered() {
    let registry = default_assist_registry();
    let names: Vec<String> = registry.definitions().into_iter().map(|d| d.name).collect();
    assert!(names.iter().any(|n| n == "get_v3_pool"));
    assert!(names.iter().any(|n| n == "quote_v3_swap"));
    assert!(names.iter().any(|n| n == "propose_v3_swap"));
    assert!(names.iter().any(|n| n == "propose_v3_mint"));
    assert!(names.iter().any(|n| n == "list_v3_positions"));
    assert!(names.iter().any(|n| n == "propose_wrap"));
    assert!(names.iter().any(|n| n == "propose_revoke"));
}
