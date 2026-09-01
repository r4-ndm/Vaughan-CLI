//! Headless LP smoke tests — error copy, fee-tier UX, job status (no live RPC).
#[cfg(test)]
mod smoke_tests {
    use std::str::FromStr;

    use vaughan_core::core::lp_smoke::LP_SMOKE_943;
    use vaughan_core::core::{V3LpPoolQuote, V3PoolLifecycle};
    use vaughan_core::error::WalletError;

    use crate::views::lp::helpers::{
        fee_tier_display, lp_network_user_message, lp_tx_error_message,
    };
    use crate::views::lp::types::{AddStep, LpDeployLastStep};
    use crate::views::LpView;

    const JIM: &str = "0xc6ca0621683db4a03e31ad77e1d63eb3a03acbba";
    const JANE: &str = "0x28Bc040cE32d78aFACb214f5460Adc2bbdaC6B59";

    #[test]
    fn lp_smoke_catalog_has_jim_jane_fee_mismatch() {
        let jim_jane = LP_SMOKE_943
            .iter()
            .find(|p| p.label == "JIM/JANE")
            .expect("catalog entry");
        assert_eq!(jim_jane.fee, 100);
        assert_eq!(jim_jane.tui_default_fee, 500);
    }

    #[test]
    fn lp_network_user_message_timeout_not_generic_connection() {
        let err = WalletError::NetworkError("LP deploy step timed out (30s)".into());
        let msg = lp_network_user_message(&err);
        assert!(
            msg.contains("timed out") || msg.contains("Pool/RPC"),
            "{msg}"
        );
        assert!(!msg.contains("Check your connection and RPC URL"), "{msg}");
    }

    #[test]
    fn lp_network_user_message_pool_missing_suggests_fee_tier() {
        let err = WalletError::NetworkError("pool does not exist for this pair/fee".into());
        let msg = lp_network_user_message(&err);
        assert!(msg.contains("fee tier") || msg.contains("0.01%"), "{msg}");
    }

    #[test]
    fn lp_network_user_message_allowance_not_generic_connection() {
        let err = WalletError::NetworkError("allowance: transport error".into());
        let msg = lp_network_user_message(&err);
        assert!(msg.contains("allowance"), "{msg}");
    }

    #[test]
    fn lp_network_user_message_getpool_keeps_rpc_hint() {
        let err = WalletError::NetworkError("getPool: connection refused".into());
        let msg = lp_network_user_message(&err);
        assert!(msg.contains("Could not reach the network"), "{msg}");
    }

    #[test]
    fn headless_quote_switches_fee_tier_from_suggestion() {
        let mut v = LpView::for_chain(943);
        assert_eq!(v.fee_tier, 500, "wiz4rd 943 default");
        v.token0.set_value(JIM);
        v.token1.set_value(JANE);
        v.add_step = AddStep::PriceDeposit;
        v.apply_pool_quote(V3LpPoolQuote {
            lifecycle: V3PoolLifecycle::Ready,
            sqrt_price_x96: None,
            tick: Some(0),
            pool_price_token1_per_token0: Some("1.66658139874".into()),
            suggested_fee_tier: Some(100),
        });
        assert_eq!(v.fee_tier, 100);
        assert!(
            v.status.contains(&fee_tier_display(100)),
            "status: {}",
            v.status
        );
        assert!(
            v.status.contains("fee tier updated"),
            "status: {}",
            v.status
        );
    }

    #[test]
    fn headless_wrong_fee_without_suggestion_stays_on_create_pool_path() {
        let mut v = LpView::for_chain(943);
        v.token0.set_value(JIM);
        v.token1.set_value(JANE);
        v.add_step = AddStep::PriceDeposit;
        v.fee_tier = 500;
        v.apply_pool_quote(V3LpPoolQuote {
            lifecycle: V3PoolLifecycle::Missing,
            sqrt_price_x96: None,
            tick: None,
            pool_price_token1_per_token0: None,
            suggested_fee_tier: None,
        });
        assert_eq!(v.fee_tier, 500);
        assert_eq!(v.pool_lifecycle, Some(V3PoolLifecycle::Missing));
    }

    #[test]
    fn lp_tx_error_message_uses_network_mapper_for_deploy_timeout() {
        let err = WalletError::NetworkError("LP deploy step timed out (30s)".into());
        let msg = lp_tx_error_message(&err, LpDeployLastStep::AddLiquidity);
        assert!(msg.contains("timed out"), "{msg}");
    }

    #[test]
    fn sorted_pair_jim_jane_maps_ui_fields_to_chain_order() {
        let mut v = LpView::for_chain(943);
        v.token0.set_value(JIM);
        v.token1.set_value(JANE);
        let pair = v.sorted_pair().expect("pair");
        assert_eq!(
            pair.token0,
            alloy::primitives::Address::from_str(JANE).unwrap()
        );
        assert_eq!(
            pair.token1,
            alloy::primitives::Address::from_str(JIM).unwrap()
        );
        assert!(!pair.first_is_token0);
    }
}
