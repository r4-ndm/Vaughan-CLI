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

use super::types::LpDeployLastStep;

pub(crate) fn format_unit_price(value: &str, sym0: &str, sym1: &str) -> String {
    format!("1 {sym0} = {value} {sym1}")
}

pub(crate) fn lp_network_user_message(err: &WalletError) -> String {
    match err {
        WalletError::NetworkError(msg) => {
            if msg.contains("timed out") || msg.contains("pool lookup timed out") {
                return "Pool/RPC timed out — wait and retry, or check RPC in Settings (F1)."
                    .into();
            }
            if msg.contains("pool does not exist") {
                return "No pool at this fee tier — use ←→ to cycle fee (e.g. JIM/JANE is 0.01%)."
                    .into();
            }
            if msg.starts_with("allowance:") {
                return "Could not read token allowance — check RPC URL (F1 network) and retry."
                    .into();
            }
            if msg.contains("no RPC URL configured") {
                return msg.clone();
            }
            if msg.starts_with("getPool:")
                || msg.starts_with("get_pool_info:")
                || msg.starts_with("decode getPool:")
                || msg.contains("invalid RPC URL")
            {
                return "Could not reach the network. Check your connection and RPC URL (F1)."
                    .into();
            }
            format!("Network error — {msg}")
        }
        WalletError::RpcError(msg) if msg.contains("all LP RPC") || msg.contains("all RPC") => {
            "All RPC endpoints failed — try another URL in Settings (F1).".into()
        }
        WalletError::RpcError(msg) => format!("RPC error — {msg}"),
        _ => err.user_message(),
    }
}

pub(crate) fn lp_tx_error_message(err: &WalletError, step: LpDeployLastStep) -> String {
    match err {
        WalletError::GasEstimationFailed(detail) => lp_gas_error_message(detail, step),
        WalletError::TransactionFailed(detail) if !detail.trim().is_empty() => {
            format!("{} · {}", err.user_message(), detail.trim())
        }
        _ => lp_network_user_message(err),
    }
}

pub(crate) fn lp_gas_error_message(detail: &str, step: LpDeployLastStep) -> String {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("already") || lower.contains("exist") {
        return match step {
            LpDeployLastStep::CreatePool => {
                "createPool would revert: pool already exists — wait for confirmation, then retry from the deposit step (Enter)."
                    .into()
            }
            _ => format!(
                "Simulation reverted (pool step may already be done). Wait for confirmation, then retry. ({detail})"
            ),
        };
    }
    if lower.contains("insufficient funds") || lower.contains("insufficient balance") {
        return "Not enough tPLS/PLS for this step + gas — lower deposit amounts or fund the wallet."
            .into();
    }
    let action = match step {
        LpDeployLastStep::CreatePool => "createPool",
        LpDeployLastStep::Initialize => "initialize",
        LpDeployLastStep::Approve => "approve",
        LpDeployLastStep::AddLiquidity => "add liquidity",
        LpDeployLastStep::None => "this LP step",
    };
    format!(
        "Could not estimate gas for {action} (on-chain simulation reverted). \
         Check token balances, fee tier, and range/amounts. ({detail})"
    )
}

pub(crate) fn lp_fee_estimate_error(err: &WalletError) -> String {
    match err {
        WalletError::GasEstimationFailed(detail) => {
            format!("Gas estimate failed — check amounts and pool state. ({detail})")
        }
        _ => lp_network_user_message(err),
    }
}

pub(crate) fn unit_price_input_line(
    input: &Input,
    quote_sym1: &str,
    focused: bool,
) -> Line<'static> {
    let suffix = format!(" {quote_sym1}");
    let suffix_span = Span::styled(suffix.clone(), Style::default().fg(Color::DarkGray));
    if input.value().is_empty() {
        if focused {
            let mut line = input.line();
            line.push_span(suffix_span);
            return line;
        }
        return Line::from(Span::styled(
            format!("{}{}", input.placeholder(), suffix),
            Style::default().fg(Color::DarkGray),
        ));
    }
    let mut line = input.line();
    line.push_span(suffix_span);
    line
}

pub(crate) fn render_unit_price_input(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    quote_sym1: &str,
    input: &Input,
    focused: bool,
    align: Alignment,
) {
    let title_text = format!(" {label} ");
    let title = if focused {
        brand::focus_title(&title_text)
    } else {
        brand::fade_line(&title_text)
    };
    let inner = brand::render_labeled_input_box(frame, area, Some(title), focused);
    frame.render_widget(
        Paragraph::new(unit_price_input_line(input, quote_sym1, focused)).alignment(align),
        inner,
    );
}

/// 9inch V3 fee tiers on Pulse (0.01% … 2%).
pub(crate) fn fee_tier_display(fee: u32) -> String {
    let pct = fee as f64 / 10_000.0;
    if pct >= 1.0 {
        format!("{pct:.1}%")
    } else {
        format!("{pct:.2}%")
    }
}

pub(crate) fn parse_price_f64(raw: &str) -> Result<f64, ()> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(());
    }
    s.parse::<f64>()
        .map_err(|_| ())
        .and_then(|p| if p > 0.0 { Ok(p) } else { Err(()) })
}

pub(crate) fn trim_float_string(v: f64) -> String {
    let s = format!("{v:.12}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Ticker for a pool token (catalog hints, then short hex).
pub(crate) fn symbol_for_token_address(chain_id: u64, addr: alloy::primitives::Address) -> String {
    let raw = format!("{addr:#x}");
    if let Some(sym) = crate::views::token_symbol_hint(&raw, chain_id) {
        return sym.to_string();
    }
    format!("{}…", &raw[2..8.min(raw.len())])
}

pub(crate) fn friendly_deploy_action(label: &str) -> String {
    match label {
        "createPool" => "Create pool".into(),
        "initialize" => "Set starting price".into(),
        "add liquidity" => "Add liquidity".into(),
        s if s.starts_with("approve") => {
            if let Some(rest) = s.strip_prefix("approve ") {
                format!("Approve {rest}")
            } else {
                "Approve token".into()
            }
        }
        _ => label.to_string(),
    }
}

/// Human pair label for a V3 position (UI field order when catalog ranks apply).
pub(crate) fn v3_position_pair_label(
    chain_id: u64,
    token0: alloy::primitives::Address,
    token1: alloy::primitives::Address,
) -> String {
    let (first, second) = pair_tokens_for_display(chain_id, token0, token1);
    format!(
        "{}/{}",
        symbol_for_token_address(chain_id, first),
        symbol_for_token_address(chain_id, second)
    )
}

fn pair_tokens_for_display(
    chain_id: u64,
    token0: alloy::primitives::Address,
    token1: alloy::primitives::Address,
) -> (alloy::primitives::Address, alloy::primitives::Address) {
    let raw0 = format!("{token0:#x}");
    let raw1 = format!("{token1:#x}");
    let rank0 = crate::views::token_lp_display_rank(&raw0, chain_id);
    let rank1 = crate::views::token_lp_display_rank(&raw1, chain_id);
    match (rank0, rank1) {
        (Some(r0), Some(r1)) if r0 != r1 => {
            if r0 < r1 {
                (token0, token1)
            } else {
                (token1, token0)
            }
        }
        (Some(_), None) => (token0, token1),
        (None, Some(_)) => (token1, token0),
        _ => (token0, token1),
    }
}
