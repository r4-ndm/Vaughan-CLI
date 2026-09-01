//! Fork E2E: LP Brew batch plan on Pulse testnet fork (skips if RPC down).

mod common;

use common::{funded_wallet_at, ForkedAnvil};
use vaughan_core::core::lp_smoke::{LP_SMOKE_943, LP_SMOKE_943_VENUE};
use vaughan_core::core::V3LpDeployParams;

#[test]
fn fork_lp_deploy_batch_plan_for_existing_smoke_pool() {
    let Some(anvil) = ForkedAnvil::start() else {
        eprintln!("testnet RPC unreachable — skipping LP batch fork smoke");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet_at(dir.path(), &anvil.url());
    let from = wallet.active_address().unwrap().to_string();
    let pair = &LP_SMOKE_943[0]; // JIM/JANE
    let token0: alloy::primitives::Address = pair.token0.parse().unwrap();
    let token1: alloy::primitives::Address = pair.token1.parse().unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let params = V3LpDeployParams {
            from,
            venue: LP_SMOKE_943_VENUE,
            chain_id: 943,
            rpc_url: anvil.url(),
            token0,
            token1,
            fee: pair.fee,
            dec0: 18,
            dec1: 18,
            pool_initial_price: "10".into(),
            pool_min_price: String::new(),
            pool_max_price: String::new(),
            amount0: "1".into(),
            amount1: String::new(),
            deposit_on_token0: true,
        };
        let plan = vaughan_core::core::build_lp_deploy_batch_calls(&params)
            .await
            .expect("batch on existing pool");
        assert!(
            !plan.steps.iter().any(|s| s == "createPool"),
            "existing pool must skip createPool: {:?}",
            plan.steps
        );
        assert!(
            plan.steps.iter().any(|s| s.contains("add liquidity")),
            "must include mint: {:?}",
            plan.steps
        );
    });
}
