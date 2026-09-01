//! Anvil integration tests for the V3 LP deploy pipeline (`vaughan_core::core::dex_lp`).
//!
//! Exercises lifecycle detection, pool quotes, fee discovery, deploy-step builders,
//! deploy-wait polling, NPM enable checks, and RPC fallback.
//!
//! ```sh
//! cargo test -p vaughan-tui --test lp_workflow_anvil -- --nocapture
//! ```

mod common;

use alloy::primitives::Address;
use alloy::sol_types::SolCall;
use common::lp_anvil_fixtures::{
    deploy_params, factory_addr, plant_erc20_allowance, plant_erc20_with_approve,
    plant_factory_missing, plant_factory_pool, plant_ready_pool_fixture, plant_v3_pool,
    plant_v3_pool_uninitialized, MOCK_POOL, TOKEN0, TOKEN1,
};
use common::{anvil_dev_address, Anvil};
use std::time::Duration;
use tokio::runtime::Runtime;
use vaughan_core::core::{
    discover_v3_pool_fee_tier, fetch_v3_lp_pool_quote, v3_lp_prepare_deploy_step,
    v3_lp_run_deploy_wait, v3_lp_token_enable_status, v3_pool_lifecycle, with_lp_rpc_urls,
    DexVenue, V3LpDeployContext, V3LpDeployWait, V3PoolLifecycle, V3_LP_FEE_TIERS,
};
use wiz4rd_sdk::abi::{IPancakeV3Factory, IPancakeV3Pool};

#[test]
fn anvil_v3_pool_lifecycle_missing() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_factory_pool(&anvil, Address::ZERO);

    let life = rt
        .block_on(v3_pool_lifecycle(
            &anvil.url(),
            DexVenue::Wiz4rd,
            943,
            TOKEN0,
            TOKEN1,
            500,
        ))
        .expect("lifecycle");
    assert_eq!(life, V3PoolLifecycle::Missing);
}

#[test]
fn anvil_v3_pool_lifecycle_ready() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_ready_pool_fixture(&anvil);

    let life = rt
        .block_on(v3_pool_lifecycle(
            &anvil.url(),
            DexVenue::Wiz4rd,
            943,
            TOKEN0,
            TOKEN1,
            500,
        ))
        .expect("lifecycle");
    assert_eq!(life, V3PoolLifecycle::Ready);
}

#[test]
fn anvil_v3_pool_lifecycle_uninitialized() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_factory_pool(&anvil, MOCK_POOL);
    plant_v3_pool(
        &anvil,
        MOCK_POOL,
        alloy::primitives::aliases::U160::ZERO,
        0,
        false,
    );

    let life = rt
        .block_on(v3_pool_lifecycle(
            &anvil.url(),
            DexVenue::Wiz4rd,
            943,
            TOKEN0,
            TOKEN1,
            500,
        ))
        .expect("lifecycle");
    assert_eq!(life, V3PoolLifecycle::Uninitialized { pool: MOCK_POOL });
}

#[test]
fn anvil_discover_v3_pool_fee_tier_first_catalog_tier() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_factory_pool(&anvil, MOCK_POOL);

    let found = rt
        .block_on(discover_v3_pool_fee_tier(
            &anvil.url(),
            DexVenue::Wiz4rd,
            943,
            TOKEN0,
            TOKEN1,
        ))
        .expect("discover");
    assert_eq!(found, Some(V3_LP_FEE_TIERS[0]));
}

#[test]
fn anvil_discover_v3_pool_fee_tier_none_when_all_missing() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_factory_pool(&anvil, Address::ZERO);

    let found = rt
        .block_on(discover_v3_pool_fee_tier(
            &anvil.url(),
            DexVenue::Wiz4rd,
            943,
            TOKEN0,
            TOKEN1,
        ))
        .expect("discover");
    assert_eq!(found, None);
}

#[test]
fn anvil_fetch_v3_lp_pool_quote_ready_includes_price() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_ready_pool_fixture(&anvil);

    let quote = rt
        .block_on(fetch_v3_lp_pool_quote(
            &anvil.url(),
            DexVenue::Wiz4rd,
            943,
            TOKEN0,
            TOKEN1,
            18,
            18,
            500,
        ))
        .expect("pool quote");
    assert_eq!(quote.lifecycle, V3PoolLifecycle::Ready);
    assert!(quote.sqrt_price_x96.is_some());
    assert!(quote.tick.is_some());
    assert!(quote.pool_price_token1_per_token0.is_some());
    assert!(quote.suggested_fee_tier.is_none());
}

#[test]
fn anvil_v3_lp_prepare_deploy_step_create_pool() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_factory_pool(&anvil, Address::ZERO);
    let from = anvil_dev_address(0);
    let params = deploy_params(&anvil, &from);

    let (tx, label) = rt
        .block_on(v3_lp_prepare_deploy_step(&params))
        .expect("createPool step");
    assert_eq!(label, "createPool");
    assert_eq!(
        tx.to.to_lowercase(),
        factory_addr().to_string().to_lowercase()
    );
    let data = tx.data.as_deref().unwrap_or("");
    assert!(
        data.starts_with(&format!(
            "0x{}",
            hex::encode(IPancakeV3Factory::createPoolCall::SELECTOR)
        )),
        "expected createPool selector in {data}"
    );
}

#[test]
fn anvil_v3_lp_prepare_deploy_step_initialize() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_factory_pool(&anvil, MOCK_POOL);
    plant_v3_pool(
        &anvil,
        MOCK_POOL,
        alloy::primitives::aliases::U160::ZERO,
        0,
        true,
    );
    let from = anvil_dev_address(0);
    let params = deploy_params(&anvil, &from);

    let (tx, label) = rt
        .block_on(v3_lp_prepare_deploy_step(&params))
        .expect("initialize step");
    assert_eq!(label, "initialize");
    assert_eq!(
        tx.to.to_lowercase(),
        format!("{MOCK_POOL:#x}").to_lowercase()
    );
    let data = tx.data.as_deref().unwrap_or("");
    assert!(
        data.starts_with(&format!(
            "0x{}",
            hex::encode(IPancakeV3Pool::initializeCall::SELECTOR)
        )),
        "expected initialize selector in {data}"
    );
}

#[test]
fn anvil_v3_lp_prepare_deploy_step_approve_when_ready() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_ready_pool_fixture(&anvil);
    plant_erc20_allowance(&anvil, TOKEN0, 0);
    plant_erc20_allowance(&anvil, TOKEN1, 0);
    let from = anvil_dev_address(0);
    let params = deploy_params(&anvil, &from);

    let (tx, label) = rt
        .block_on(v3_lp_prepare_deploy_step(&params))
        .expect("approve step");
    assert!(
        label.starts_with("approve "),
        "expected approve label, got {label}"
    );
    let data = tx.data.as_deref().unwrap_or("");
    assert!(
        data.starts_with("0x095ea7b3"),
        "expected ERC-20 approve selector in {data}"
    );
}

#[test]
fn anvil_v3_lp_token_enable_status_reports_missing_allowances() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_ready_pool_fixture(&anvil);
    plant_erc20_allowance(&anvil, TOKEN0, 0);
    plant_erc20_allowance(&anvil, TOKEN1, 0);
    let from = anvil_dev_address(0);
    let params = deploy_params(&anvil, &from);

    let status = rt
        .block_on(v3_lp_token_enable_status(&params))
        .expect("enable status")
        .expect("ready pool should report enable tuple");
    assert_eq!(status, (false, false));
}

#[test]
fn anvil_v3_lp_token_enable_status_skips_missing_pool() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_factory_pool(&anvil, Address::ZERO);
    let from = anvil_dev_address(0);
    let params = deploy_params(&anvil, &from);

    let status = rt
        .block_on(v3_lp_token_enable_status(&params))
        .expect("enable status");
    assert!(status.is_none());
}

#[test]
fn anvil_with_lp_rpc_urls_uses_working_fallback() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_ready_pool_fixture(&anvil);

    let urls = vec!["http://127.0.0.1:1".into(), anvil.url()];
    let life = rt
        .block_on(with_lp_rpc_urls(&urls, |url| async move {
            v3_pool_lifecycle(&url, DexVenue::Wiz4rd, 943, TOKEN0, TOKEN1, 500).await
        }))
        .expect("fallback RPC");
    assert_eq!(life, V3PoolLifecycle::Ready);
}

#[test]
fn anvil_deploy_wait_after_create_pool_polls_until_visible() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_factory_missing(&anvil);
    let from = anvil_dev_address(0);
    let params = deploy_params(&anvil, &from);

    rt.block_on(async {
        let delay = tokio::time::sleep(Duration::from_millis(2_500));
        let wait = v3_lp_run_deploy_wait(V3LpDeployWait::AfterCreatePool, &params, None);
        tokio::pin!(delay);
        tokio::pin!(wait);

        loop {
            tokio::select! {
                _ = &mut delay => {
                    plant_factory_pool(&anvil, MOCK_POOL);
                    plant_v3_pool_uninitialized(&anvil);
                }
                result = &mut wait => {
                    result.expect("createPool wait");
                    return;
                }
            }
        }
    });
}

#[test]
fn anvil_deploy_wait_after_initialize_polls_until_ready() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_factory_pool(&anvil, MOCK_POOL);
    plant_v3_pool_uninitialized(&anvil);
    let from = anvil_dev_address(0);
    let params = deploy_params(&anvil, &from);

    rt.block_on(async {
        let delay = tokio::time::sleep(Duration::from_millis(2_500));
        let wait = v3_lp_run_deploy_wait(V3LpDeployWait::AfterInitialize, &params, None);
        tokio::pin!(delay);
        tokio::pin!(wait);

        loop {
            tokio::select! {
                _ = &mut delay => {
                    plant_ready_pool_fixture(&anvil);
                }
                result = &mut wait => {
                    result.expect("initialize wait");
                    return;
                }
            }
        }
    });
}

#[test]
fn anvil_deploy_wait_after_approve_polls_until_allowance_covers_mint() {
    let anvil = Anvil::start();
    let rt = Runtime::new().unwrap();
    plant_ready_pool_fixture(&anvil);
    plant_erc20_with_approve(&anvil, TOKEN0, 0);
    plant_erc20_allowance(&anvil, TOKEN1, u64::MAX);
    let from = anvil_dev_address(0);
    let params = deploy_params(&anvil, &from);
    let ctx = V3LpDeployContext {
        last_step_label: Some("approve token0 for LP".into()),
    };

    rt.block_on(async {
        let delay = tokio::time::sleep(Duration::from_millis(2_500));
        let wait = v3_lp_run_deploy_wait(V3LpDeployWait::AfterApprove, &params, Some(&ctx));
        tokio::pin!(delay);
        tokio::pin!(wait);

        loop {
            tokio::select! {
                _ = &mut delay => {
                    plant_erc20_with_approve(&anvil, TOKEN0, u64::MAX);
                }
                result = &mut wait => {
                    result.expect("approve wait");
                    return;
                }
            }
        }
    });
}
