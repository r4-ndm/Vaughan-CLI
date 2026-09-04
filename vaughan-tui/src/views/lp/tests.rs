#![allow(unused_imports)]
use alloy::primitives::U256;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use std::str::FromStr;
use tokio::runtime::Handle;
use vaughan_core::chains::Balance;
use vaughan_core::core::wiz4rd::WZRD_SMOKE_943;
use vaughan_core::core::{
    build_v2_add_liquidity_evm, build_v2_remove_liquidity_evm, build_v3_collect_evm,
    build_v3_decrease_evm, build_v3_increase_evm, chain_label, default_full_range_ticks,
    display_price_range_from_preset, format_display_amount, lp_stack_for_chain, lp_v3_venue_picker,
    min_out_after_slippage, v3_preview_mint_deposits_from_amount0,
    v3_preview_mint_deposits_from_amount1, v3_range_ticks_from_human_prices,
    v3_sqrt_and_tick_for_preview, venue_position_manager, venue_swap_router, wpls_for_chain,
    DexProtocol, DexVenue, LpStack, V2LpPosition, V3LpDeployWait, V3PoolLifecycle, V3PositionInfo,
    WalletState, DEFAULT_DEX_SLIPPAGE_BPS,
};
use vaughan_core::error::WalletError;
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::swap_form::SWAP_DISPLAY_FRAC;
use crate::views::{
    body_areas, cycle_token_picker, manual_edit_resets_token_pick, render_labeled_input,
    render_labeled_input_aligned, status_paragraph, token_symbol_for_address, TOKEN_PICK_UNINIT,
};
use crate::views::{parse_swap_amount, parse_token_address};

use super::helpers::fee_tier_display;
use super::types::*;
use super::LpView;

mod tests {
    use super::*;

    #[test]
    fn fee_tier_display_matches_9inch_labels() {
        assert_eq!(fee_tier_display(100), "0.01%");
        assert_eq!(fee_tier_display(500), "0.05%");
        assert_eq!(fee_tier_display(2500), "0.25%");
        assert_eq!(fee_tier_display(10_000), "1.0%");
        assert_eq!(fee_tier_display(20_000), "2.0%");
    }

    #[test]
    fn v3_list_labels_are_human() {
        use super::super::helpers::{
            fee_tier_short, short_units, v3_fees_owed_label, v3_liq_short, v3_liquidity_status,
            v3_range_label, v3_range_short,
        };
        use alloy::primitives::U256;
        assert_eq!(v3_liquidity_status(0), "Empty");
        assert_eq!(v3_liquidity_status(1), "Dust");
        assert_eq!(v3_liquidity_status(2), "Active");
        assert_eq!(v3_liq_short(0), "Emp");
        assert_eq!(v3_liq_short(1), "Dst");
        assert_eq!(v3_liq_short(2), "Act");
        assert_eq!(v3_range_label(-887_200, 887_200), "Full range");
        assert_eq!(v3_range_label(-85_200, -71_600), "Custom");
        assert_eq!(v3_range_short(-887_200, 887_200), "Full");
        assert_eq!(v3_range_short(-85_200, -71_600), "Cust");
        assert_eq!(v3_fees_owed_label(0, 0), "—");
        assert_eq!(v3_fees_owed_label(1, 0), "Ready");
        assert_eq!(fee_tier_short(20_000), "2%");
        assert_eq!(fee_tier_short(500), ".05");
        assert_eq!(short_units(U256::from(42u64)), "42");
        assert!(short_units(U256::from(1_983_098_040_965_341_809u64)).contains('e'));
    }

    #[test]
    fn v3_opens_on_list_tab() {
        let v = LpView::for_chain(369);
        assert_eq!(v.tab, Tab::List);
        assert_eq!(v.add_step, AddStep::SelectPair);
    }

    #[test]
    fn up_down_cycles_lp_venue_from_default_focus() {
        let mut v = LpView::for_chain(369);
        v.tab = Tab::AddLp;
        assert_eq!(v.venue, DexVenue::NineInch);
        assert!(v.cycle_venue_selector(true));
        assert_eq!(v.venue, DexVenue::NineMm);
    }

    #[test]
    fn nine_mm_default_range_matches_50_percent_url() {
        let v = LpView::for_chain(369);
        assert_eq!(v.venue, DexVenue::NineInch);
        let mut v = v;
        v.venue = DexVenue::NineMm;
        v.apply_venue_token_defaults(true);
        assert_eq!(v.range_preset_applied, Some(5));
        let min: f64 = v.min_price.value().parse().unwrap();
        let max: f64 = v.max_price.value().parse().unwrap();
        assert!((min - 0.000_632_662_25).abs() < 1e-9);
        assert!((max - 0.002_530_649).abs() < 1e-9);
    }

    #[test]
    fn range_preset_fifty_percent_matches_9mm_url() {
        let mut v = LpView::for_chain(369);
        v.initial_price.set_value("0.001265324");
        v.apply_range_preset(5);
        let min: f64 = v.min_price.value().parse().unwrap();
        let max: f64 = v.max_price.value().parse().unwrap();
        assert!((min - 0.000_632_662_25).abs() < 1e-9);
        assert!((max - 0.002_530_649).abs() < 1e-9);
    }

    #[test]
    fn range_preset_ten_percent_symmetric() {
        let mut v = LpView::for_chain(369);
        v.initial_price.set_value("1");
        v.apply_range_preset(3);
        assert_eq!(v.min_price.value(), "0.9");
        assert_eq!(v.max_price.value(), "1.1");
    }

    #[test]
    fn v3_custom_range_starts_collapsed() {
        let v = LpView::for_chain(369);
        assert!(!v.v3_custom_range);
    }

    #[test]
    fn preset_first_tab_skips_min_max_until_custom_open() {
        let mut v = LpView::for_chain(369);
        v.add_step = AddStep::PriceDeposit;
        v.pool_lifecycle = Some(V3PoolLifecycle::Ready);
        v.focus = Focus::None;
        assert_eq!(v.focus_tab_forward(), Focus::RangePresets);
        v.focus = Focus::RangePresets;
        assert_eq!(v.focus_tab_forward(), Focus::Amount0);
        v.focus = Focus::Amount0;
        assert_eq!(v.focus_tab_forward(), Focus::Amount1);
    }

    #[test]
    fn custom_range_tab_includes_min_max_fields() {
        let mut v = LpView::for_chain(369);
        v.add_step = AddStep::PriceDeposit;
        v.v3_custom_range = true;
        v.focus = Focus::RangePresets;
        assert_eq!(v.focus_tab_forward(), Focus::MinPrice);
        v.focus = Focus::MinPrice;
        assert_eq!(v.focus_tab_forward(), Focus::InitialPrice);
    }

    #[test]
    fn simple_mode_deposit_guidance_when_in_range() {
        use vaughan_core::core::sqrt_price_x96_from_tick;
        let mut v = LpView::for_chain(943);
        v.add_step = AddStep::PriceDeposit;
        v.initial_price.set_value("1");
        v.apply_range_preset(5);
        v.pool_lifecycle = Some(V3PoolLifecycle::Ready);
        v.pool_tick = Some(0);
        v.pool_sqrt_x96 = Some(sqrt_price_x96_from_tick(0).unwrap());
        let hint = v.v3_deposit_guidance("A", "B");
        assert!(hint.contains("fills in automatically"), "{hint}");
    }

    #[test]
    fn toggle_custom_range() {
        let mut v = LpView::for_chain(369);
        assert!(!v.v3_custom_range);
        v.toggle_v3_custom_range();
        assert!(v.v3_custom_range);
        v.toggle_v3_custom_range();
        assert!(!v.v3_custom_range);
    }

    #[test]
    fn tab_from_presets_reaches_both_deposits() {
        let mut v = LpView::for_chain(369);
        v.add_step = AddStep::PriceDeposit;
        v.pool_lifecycle = Some(V3PoolLifecycle::Ready);
        v.range_preset_applied = Some(5);
        v.focus = Focus::RangePresets;
        assert_eq!(v.focus_tab_forward(), Focus::Amount0);
        v.focus = Focus::Amount0;
        assert_eq!(v.focus_tab_forward(), Focus::Amount1);
    }

    #[test]
    fn normalize_focus_skips_hidden_initial_price() {
        let mut v = LpView::for_chain(369);
        v.add_step = AddStep::PriceDeposit;
        v.pool_lifecycle = Some(V3PoolLifecycle::Ready);
        v.focus = Focus::InitialPrice;
        v.normalize_v3_price_focus();
        assert_eq!(v.focus, Focus::Amount0);
    }

    #[test]
    fn preset_hotkey_focuses_starting_price_on_new_pool() {
        let mut v = LpView::for_chain(369);
        v.add_step = AddStep::PriceDeposit;
        v.pool_lifecycle = Some(V3PoolLifecycle::Missing);
        v.apply_range_preset(5);
        assert_eq!(v.range_preset_applied, Some(5));
        assert_eq!(v.focus, Focus::InitialPrice);
    }

    #[test]
    fn tab_from_none_reaches_presets_even_when_already_applied() {
        let mut v = LpView::for_chain(369);
        v.add_step = AddStep::PriceDeposit;
        v.pool_lifecycle = Some(V3PoolLifecycle::Ready);
        v.range_preset_applied = Some(5);
        v.focus = Focus::None;
        assert_eq!(v.focus_tab_forward(), Focus::RangePresets);
        v.focus = Focus::RangePresets;
        assert_eq!(v.focus_tab_forward(), Focus::Amount0);
    }

    #[test]
    fn pool_quote_restores_status_after_preset_applied() {
        use vaughan_core::core::V3LpPoolQuote;
        let mut v = LpView::for_chain(369);
        v.add_step = AddStep::PriceDeposit;
        v.apply_range_preset(5);
        v.pool_quote_inflight = true;
        v.status.push_str(" · verifying on chain…");
        v.apply_pool_quote(V3LpPoolQuote {
            lifecycle: V3PoolLifecycle::Missing,
            sqrt_price_x96: None,
            tick: None,
            pool_price_token1_per_token0: None,
            suggested_fee_tier: None,
        });
        assert!(v.status.contains("Range 50%"));
        assert!(!v.status.contains("verifying"));
        assert!(!v.pool_quote_inflight);
    }

    #[test]
    fn stale_pool_quote_ignored_after_leaving_step() {
        use vaughan_core::core::V3LpPoolQuote;
        let mut v = LpView::for_chain(369);
        v.add_step = AddStep::PriceDeposit;
        v.begin_optimistic_pool_preview();
        v.pool_quote_inflight = true;
        v.add_step = AddStep::SelectPair;
        v.apply_pool_quote(V3LpPoolQuote {
            lifecycle: V3PoolLifecycle::Ready,
            sqrt_price_x96: None,
            tick: Some(0),
            pool_price_token1_per_token0: Some("1".into()),
            suggested_fee_tier: None,
        });
        assert!(!v.pool_quote_inflight);
        assert_eq!(v.pool_lifecycle, Some(V3PoolLifecycle::Missing));
    }

    #[test]
    fn starting_price_resyncs_preset_bounds() {
        let mut v = LpView::for_chain(369);
        v.apply_range_preset(5);
        v.initial_price.set_value("0.005");
        v.resync_range_bounds_from_preset();
        let min: f64 = v.min_price.value().parse().unwrap();
        let max: f64 = v.max_price.value().parse().unwrap();
        assert!((min - 0.002_5).abs() < 1e-9);
        assert!((max - 0.01).abs() < 1e-9);
    }

    #[test]
    fn range_preset_full_clears_bounds() {
        let mut v = LpView::for_chain(369);
        v.min_price.set_value("0.1");
        v.max_price.set_value("2");
        v.apply_range_preset(RANGE_PRESETS.len() - 1);
        assert!(v.min_price.value().trim().is_empty());
        assert!(v.max_price.value().trim().is_empty());
    }

    #[test]
    fn deploy_step_ready_updates_status_from_checking_pool() {
        use vaughan_core::chains::EvmTransaction;
        let mut v = LpView::for_chain(943);
        v.lp_pipeline_phase = LpPipelinePhase::Review;
        v.busy = Busy::Loading;
        v.status = "Checking pool…".into();
        v.apply_job_result(UiJobResult::LpV3PoolDeployStep(Ok((
            EvmTransaction {
                from: "0x1".into(),
                to: "0x2".into(),
                value: "0".into(),
                data: None,
                gas_limit: None,
                gas_price: None,
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                nonce: None,
                chain_id: 943,
            },
            "createPool".into(),
        ))));
        assert_eq!(v.busy, Busy::EstimatingFee);
        assert!(v.status.contains("Review deposit"));
        assert_eq!(v.stage, Stage::Confirm);
        assert!(v
            .confirm_ui
            .as_ref()
            .map(|ui| matches!(ui.action, LpConfirmAction::AddReview))
            .unwrap_or(false));
        assert!(v
            .confirm_ui
            .as_ref()
            .and_then(|ui| ui.pending_fee_estimate.as_ref())
            .is_some());
    }

    #[test]
    fn inverted_pair_range_maps_to_ascending_pool_prices() {
        use alloy::primitives::Address;
        use std::str::FromStr;
        use vaughan_core::core::v3_range_ticks_from_human_prices;

        let mut v = LpView::for_chain(943);
        // JANE (token1) before BOB (token0) — UI price is BOB per JANE.
        let jane = "0x28Bc040cE32d78aFACb214f5460Adc2bbdaC6B59";
        let bob = "0x15de8ae884726f37ec90824f825d723ac93c8b77";
        v.token0.set_value(jane);
        v.token1.set_value(bob);
        v.fee_tier = 20_000;
        v.initial_price.set_value("5.91715976");
        v.apply_range_preset(1); // 2%

        let pair = v.sorted_pair().expect("pair");
        assert!(!pair.first_is_token0);

        let (pool_min, pool_max) = v
            .user_price_range_to_pool_prices(
                pair.first_is_token0,
                v.min_price.value(),
                v.max_price.value(),
            )
            .expect("range");
        let min_f: f64 = pool_min.parse().unwrap();
        let max_f: f64 = pool_max.parse().unwrap();
        assert!(
            min_f < max_f,
            "pool min {pool_min} must be below max {pool_max}"
        );

        let (lo, hi) = v3_range_ticks_from_human_prices(
            943,
            Address::from_str(bob).unwrap(),
            Address::from_str(jane).unwrap(),
            18,
            18,
            &pool_min,
            &pool_max,
            20_000,
        )
        .expect("ticks");
        assert!(lo < hi);
    }

    #[test]
    fn v3_position_pair_label_uses_catalog_hints() {
        use alloy::primitives::Address;
        use std::str::FromStr;
        let bob = Address::from_str("0x15de8ae884726f37ec90824f825d723ac93c8b77").unwrap();
        let jim = Address::from_str("0xc6ca0621683db4a03e31ad77e1d63eb3a03acbba").unwrap();
        assert_eq!(
            super::super::helpers::v3_position_pair_label(943, bob, jim, &[]),
            "JIM/BOB"
        );
    }

    #[test]
    fn v3_pair_label_uses_t1_t2_plsx_hints() {
        use alloy::primitives::Address;
        use std::str::FromStr;
        let t1 = Address::from_str("0x33df366093ef8ac488e5be40e7ee2eeac2142770").unwrap();
        let t2 = Address::from_str("0xfc413180d3624349d111fd98ee76bc08a25bc655").unwrap();
        let jane = Address::from_str("0x28Bc040cE32d78aFACb214f5460Adc2bbdaC6B59").unwrap();
        let plsx = Address::from_str("0x8a810ea8b121d08342e9e7696f4a9915cbe494b7").unwrap();
        assert_eq!(
            super::super::helpers::v3_position_pair_label(943, t1, t2, &[]),
            "T1/T2"
        );
        assert_eq!(
            super::super::helpers::v3_position_pair_label(943, jane, plsx, &[]),
            "JANE/PLSX"
        );
    }

    #[test]
    fn decrease_defaults_to_partial_not_full_position() {
        use alloy::primitives::{Address, U256};
        use std::str::FromStr;
        use vaughan_core::core::{V3LpPositionView, V3PositionInfo};

        let mut v = LpView::for_chain(943);
        v.v3_positions
            .push(V3LpPositionView::from_info_only(V3PositionInfo {
                token_id: U256::from(4u64),
                nonce: 0,
                operator: Address::ZERO,
                token0: Address::from_str("0x15de8ae884726f37ec90824f825d723ac93c8b77").unwrap(),
                token1: Address::from_str("0xc6ca0621683db4a03e31ad77e1d63eb3a03acbba").unwrap(),
                fee: 20_000,
                tick_lower: 0,
                tick_upper: 0,
                liquidity: 1_000,
                fee_growth_inside0_last_x128: U256::ZERO,
                fee_growth_inside1_last_x128: U256::ZERO,
                tokens_owed0: 0,
                tokens_owed1: 0,
            }));
        v.tab = Tab::Decrease;
        v.on_tab_changed();
        assert_eq!(v.decrease_preset_applied, Some(0));
        assert_eq!(v.liquidity.value(), "250");
        v.apply_decrease_preset(3);
        assert_eq!(v.liquidity.value(), "1000");
    }

    #[test]
    fn list_up_down_prefers_positions_with_liquidity() {
        use alloy::primitives::{Address, U256};
        use std::str::FromStr;
        use vaughan_core::core::{V3LpPositionView, V3PositionInfo};

        fn pos(id: u64, liq: u128) -> V3LpPositionView {
            V3LpPositionView::from_info_only(V3PositionInfo {
                token_id: U256::from(id),
                nonce: 0,
                operator: Address::ZERO,
                token0: Address::from_str("0x15de8ae884726f37ec90824f825d723ac93c8b77").unwrap(),
                token1: Address::from_str("0xc6ca0621683db4a03e31ad77e1d63eb3a03acbba").unwrap(),
                fee: 500,
                tick_lower: -100,
                tick_upper: 100,
                liquidity: liq,
                fee_growth_inside0_last_x128: U256::ZERO,
                fee_growth_inside1_last_x128: U256::ZERO,
                tokens_owed0: 0,
                tokens_owed1: 0,
            })
        }

        let mut v = LpView::for_chain(943);
        assert_eq!(v.tab, Tab::List);
        v.v3_positions = vec![pos(1, 0), pos(2, 500), pos(3, 0), pos(4, 900)];
        v.clamp_list_sel();
        assert_eq!(v.sel, 1, "start on first liquid row");
        v.move_list_sel(true);
        assert_eq!(v.sel, 3);
        v.move_list_sel(true);
        assert_eq!(v.sel, 1, "wrap among liquid only");
        v.move_list_sel(false);
        assert_eq!(v.sel, 3);
    }

    #[test]
    fn list_enter_opens_actions_then_manage_tab() {
        use alloy::primitives::{Address, U256};
        use std::str::FromStr;
        use vaughan_core::core::{V3LpPositionView, V3PositionInfo};

        let mut v = LpView::for_chain(943);
        v.v3_positions
            .push(V3LpPositionView::from_info_only(V3PositionInfo {
                token_id: U256::from(7u64),
                nonce: 0,
                operator: Address::ZERO,
                token0: Address::from_str("0x15de8ae884726f37ec90824f825d723ac93c8b77").unwrap(),
                token1: Address::from_str("0xc6ca0621683db4a03e31ad77e1d63eb3a03acbba").unwrap(),
                fee: 500,
                tick_lower: 0,
                tick_upper: 10,
                liquidity: 42,
                fee_growth_inside0_last_x128: U256::ZERO,
                fee_growth_inside1_last_x128: U256::ZERO,
                tokens_owed0: 0,
                tokens_owed1: 0,
            }));
        v.clamp_list_sel();
        v.open_list_actions();
        assert_eq!(v.list_action_idx, Some(0));
        assert!(v.apply_list_action_key('d'));
        assert_eq!(v.tab, Tab::Decrease);
        assert!(v.list_action_idx.is_none());

        v.tab = Tab::List;
        v.open_list_actions();
        v.enter_list_action();
        assert_eq!(v.tab, Tab::Increase, "Enter opens first action");
        assert!(v.list_action_idx.is_none());

        v.tab = Tab::List;
        v.open_list_actions();
        v.close_list_actions();
        assert!(v.list_action_idx.is_none());
    }
}
