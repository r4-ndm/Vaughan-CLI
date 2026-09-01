//! Full V3 LP deploy pipeline on Anvil: createPool → initialize → approve → mint.
//!
//! Mirrors the TUI multi-step job chain (`v3_lp_prepare_deploy_step` +
//! `v3_lp_run_deploy_wait` + `WalletState::send_transaction`).
//!
//! ```sh
//! cargo test -p vaughan-tui --test lp_pipeline_anvil -- --nocapture
//! ```

mod common;

use common::lp_anvil_fixtures::{
    deploy_params, plant_erc20_with_approve, plant_factory_missing_with_create, plant_factory_pool,
    plant_npm_mint, plant_ready_pool_fixture, plant_v3_pool_uninitialized, wait_receipt, MOCK_POOL,
    NPM, TOKEN0, TOKEN1,
};
use common::{funded_wallet, Anvil};
use tokio::runtime::Runtime;
use vaughan_core::core::wiz4rd::POSITION_MANAGER_943;
use vaughan_core::core::WalletState;
use vaughan_core::core::{
    v3_lp_prepare_deploy_step, v3_lp_run_deploy_wait, V3LpDeployContext, V3LpDeployParams,
    V3LpDeployWait,
};

async fn broadcast_step(wallet: &WalletState, anvil: &Anvil, params: &V3LpDeployParams) -> String {
    let (tx, _label) = v3_lp_prepare_deploy_step(params)
        .await
        .expect("prepare deploy step");
    let hash = wallet
        .send_transaction(tx)
        .await
        .unwrap_or_else(|e| panic!("broadcast failed: {}", e.user_message()));
    wait_receipt(anvil, &hash.to_string());
    hash.to_string()
}

#[test]
fn anvil_v3_lp_full_deploy_pipeline_broadcasts() {
    common::lp_anvil_fixtures::npm_catalog_matches();

    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let from = wallet.active_address().unwrap();
    let rt = Runtime::new().unwrap();

    plant_factory_missing_with_create(&anvil);
    plant_erc20_with_approve(&anvil, TOKEN0, 0);
    plant_erc20_with_approve(&anvil, TOKEN1, 0);
    plant_npm_mint(&anvil);

    let params = deploy_params(&anvil, &from);

    rt.block_on(async {
        // 1. createPool
        assert_eq!(
            v3_lp_prepare_deploy_step(&params).await.expect("prepare").1,
            "createPool"
        );
        broadcast_step(&wallet, &anvil, &params).await;
        plant_factory_pool(&anvil, MOCK_POOL);
        plant_v3_pool_uninitialized(&anvil);
        v3_lp_run_deploy_wait(V3LpDeployWait::AfterCreatePool, &params, None)
            .await
            .expect("wait createPool");

        // 2. initialize
        assert_eq!(
            v3_lp_prepare_deploy_step(&params).await.expect("prepare").1,
            "initialize"
        );
        broadcast_step(&wallet, &anvil, &params).await;
        plant_ready_pool_fixture(&anvil);
        v3_lp_run_deploy_wait(V3LpDeployWait::AfterInitialize, &params, None)
            .await
            .expect("wait initialize");

        // 3–4. approve token0, then token1 (infinite NPM allowance)
        for expected in ["approve token0 for LP", "approve token1 for LP"] {
            let (tx, label) = v3_lp_prepare_deploy_step(&params)
                .await
                .expect("prepare approve");
            assert_eq!(label, expected);
            let token = if expected.contains("token0") {
                TOKEN0
            } else {
                TOKEN1
            };
            let hash = wallet
                .send_transaction(tx)
                .await
                .expect("approve broadcast");
            wait_receipt(&anvil, &hash.to_string());
            plant_erc20_with_approve(&anvil, token, u64::MAX);
            let ctx = V3LpDeployContext {
                last_step_label: Some(label),
            };
            v3_lp_run_deploy_wait(V3LpDeployWait::AfterApprove, &params, Some(&ctx))
                .await
                .expect("wait approve");
        }

        // 5. mint
        let (tx, label) = v3_lp_prepare_deploy_step(&params)
            .await
            .expect("prepare mint");
        assert_eq!(label, "add liquidity");
        assert_eq!(
            tx.to.to_lowercase(),
            POSITION_MANAGER_943.to_lowercase(),
            "mint must target catalog NPM"
        );
        assert_eq!(tx.to.to_lowercase(), format!("{NPM:#x}").to_lowercase());
        let hash = wallet.send_transaction(tx).await.expect("mint broadcast");
        wait_receipt(&anvil, &hash.to_string());
    });
}
