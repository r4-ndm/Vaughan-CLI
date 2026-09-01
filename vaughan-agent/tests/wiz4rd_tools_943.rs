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
            profile_dir: None,
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

#[tokio::test]
#[ignore = "hits rpc.v4.testnet.pulsechain.com — run with --ignored when online"]
async fn wiz4rd_build_swap_and_lp_proposals_on_943() {
    let registry = default_assist_registry();
    let ctx = ToolContext {
        rpc_url: "https://rpc.v4.testnet.pulsechain.com".into(),
        chain_id: 943,
        active_address: Some(address!("0xAe089fF30590206F24E4E6627Ea61E4944cFc895")),
            profile_dir: None,
    };

    // Only WZRD/WPLS @ fee 500 is deployed on 943 today; other tiers have no pool.
    for fee in [100u64, 2500, 10000] {
        let err = registry
            .execute(
                "get_v3_pool",
                json!({ "token_a": "WPLS", "token_b": "WZRD", "fee": fee }),
                &ctx,
            )
            .await
            .expect_err(&format!("fee {fee} should have no pool"));
        assert!(
            err.to_string().contains("pool") || err.to_string().contains("decode"),
            "unexpected err for fee {fee}: {err}"
        );
    }

    let swap = registry
        .execute(
            "propose_v3_swap",
            json!({
                "token_in": "WZRD",
                "token_out": "WPLS",
                "amount_in": "10000000000000000",
                "fee": 500,
                "slippage_bps": 50,
                "explanation": "Live test: 0.01 WZRD → WPLS wiz4rd fee 500"
            }),
            &ctx,
        )
        .await
        .expect("propose_v3_swap");
    assert!(swap["calldata"].as_str().unwrap().starts_with("0x"));
    assert_eq!(
        swap["to"].as_str().unwrap().to_lowercase(),
        "0xfc656c95ecd418536844feeaa46949bb9365beaf"
    );
    // eth_call often reverts until WZRD is approved on the SwapRouter — proposal still valid.

    let mint = registry
        .execute(
            "propose_v3_mint",
            json!({
                "token_a": "WZRD",
                "token_b": "WPLS",
                "amount_a": "1000000000000000000",
                "amount_b": "1000000000000000000",
                "fee": 500,
                "range_spacings": 20,
                "slippage_bps": 50,
                "venue": "wiz4rd",
                "explanation": "Live test: wide WZRD/WPLS LP fee 500"
            }),
            &ctx,
        )
        .await
        .expect("propose_v3_mint");
    assert_eq!(mint["simulation_success"], true);
    assert!(mint["calldata"].as_str().unwrap().starts_with("0x"));
    assert_eq!(
        mint["network_id"].as_str().unwrap(),
        "pulsechain-testnet-v4"
    );
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

    let mint = registry
        .definitions()
        .into_iter()
        .find(|d| d.name == "propose_v3_mint")
        .expect("propose_v3_mint");
    assert!(mint.parameters["properties"]["venue"].is_object());
}
