//! MCP `propose_v3_lp_deploy` integration on Anvil (mirrors lp_pipeline fixtures).

mod common;

use common::lp_anvil_fixtures::{
    deploy_params, plant_erc20_with_approve, plant_factory_missing_with_create, plant_npm_mint,
    TOKEN0, TOKEN1,
};
use common::{funded_wallet, Anvil};
use serde_json::json;
use std::str::FromStr;
use tokio::runtime::Runtime;
use vaughan_agent::tools::{default_assist_registry, ToolContext};

#[test]
fn anvil_propose_v3_lp_deploy_first_step() {
    common::lp_anvil_fixtures::npm_catalog_matches();
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let from = wallet.active_address().unwrap();
    let from_addr = alloy::primitives::Address::from_str(from).unwrap();
    plant_factory_missing_with_create(&anvil);
    plant_erc20_with_approve(&anvil, TOKEN0, u64::MAX);
    plant_erc20_with_approve(&anvil, TOKEN1, u64::MAX);
    plant_npm_mint(&anvil);

    let params = deploy_params(&anvil, from);
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let registry = default_assist_registry();
        let context = ToolContext {
            rpc_url: anvil.url(),
            chain_id: 943,
            active_address: Some(from_addr),
            profile_dir: Some(dir.path().to_path_buf()),
        };
        let raw = registry
            .execute(
                "propose_v3_lp_deploy",
                json!({
                    "token_a": format!("{:#x}", params.token0),
                    "token_b": format!("{:#x}", params.token1),
                    "price": params.pool_initial_price,
                    "deposit": params.amount0,
                    "deposit_token": format!("{:#x}", params.token0),
                    "fee": params.fee,
                    "explanation": "Anvil LP Brew MCP smoke"
                }),
                &context,
            )
            .await
            .expect("propose_v3_lp_deploy");
        let step = raw["step"].as_str().expect("step label");
        assert_eq!(step, "createPool");
        let job_id = raw["job_id"].as_str().expect("job_id");
        let job_path = dir
            .path()
            .join("lp_deploy_jobs")
            .join(format!("{job_id}.json"));
        assert!(job_path.exists(), "job persisted at {}", job_path.display());
    });
}

#[test]
fn anvil_build_lp_deploy_batch_calls_missing_pool() {
    common::lp_anvil_fixtures::npm_catalog_matches();
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let from = wallet.active_address().unwrap();
    plant_factory_missing_with_create(&anvil);
    plant_erc20_with_approve(&anvil, TOKEN0, u64::MAX);
    plant_erc20_with_approve(&anvil, TOKEN1, u64::MAX);
    plant_npm_mint(&anvil);

    let params = deploy_params(&anvil, from);
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let plan = vaughan_core::core::build_lp_deploy_batch_calls(&params)
            .await
            .expect("batch plan");
        assert_eq!(plan.steps.len(), 4, "steps: {:?}", plan.steps);
        assert_eq!(plan.steps[0], "createPool");
        assert_eq!(plan.steps.last().map(String::as_str), Some("add liquidity"));
        assert!(plan.gas_warning.is_none());
        assert_eq!(plan.calls.len(), 4);
    });
}
