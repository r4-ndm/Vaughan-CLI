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
    DexProtocol, DexVenue, LpStack, V2LpPosition, V3LpDeployWait, V3LpPositionView, V3PoolLifecycle,
    V3PositionInfo, WalletState, DEFAULT_DEX_SLIPPAGE_BPS,
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
            let lower = detail.to_ascii_lowercase();
            if lower.contains("execution reverted") || lower.contains("error code 3") {
                format!(
                    "Decrease/collect simulation reverted — liquidity may already be dust. \
                     Esc · ←→ Collect, or press r to reload. ({detail})"
                )
            } else {
                format!("Gas estimate failed — check amounts and pool state. ({detail})")
            }
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

/// Ticker for a pool token (F2 assets → catalog hints → short hex).
pub(crate) fn symbol_for_token_address(
    chain_id: u64,
    addr: alloy::primitives::Address,
    assets: &[Balance],
) -> String {
    let raw = format!("{addr:#x}");
    if let Some(sym) = token_symbol_for_address(assets, &raw) {
        return sym.to_string();
    }
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
    assets: &[Balance],
) -> String {
    let (first, second) = pair_tokens_for_display(chain_id, token0, token1);
    format!(
        "{}/{}",
        symbol_for_token_address(chain_id, first, assets),
        symbol_for_token_address(chain_id, second, assets)
    )
}

/// Active liquidity vs fully withdrawn (still holds the NFT).
///
/// Uniswap-style decreases often leave `liquidity == 1` dust with tokens in
/// `tokensOwed*` — that is **Collect**, not another Decrease.
pub(crate) fn v3_liquidity_status(liquidity: u128) -> &'static str {
    match liquidity {
        0 => "Empty",
        1 => "Dust",
        _ => "Active",
    }
}

/// Compact Liq column for the position table.
pub(crate) fn v3_liq_short(liquidity: u128) -> &'static str {
    match liquidity {
        0 => "Emp",
        1 => "Dst",
        _ => "Act",
    }
}

/// Short hint when decrease is the wrong next step.
pub(crate) fn v3_manage_hint(liquidity: u128, owed0: u128, owed1: u128) -> Option<&'static str> {
    if liquidity <= 1 && (owed0 > 0 || owed1 > 0) {
        Some("Dust left · Collect owed tokens (←→ Collect)")
    } else if liquidity == 0 {
        Some("No liquidity · Collect if fees owed, or Add LP")
    } else {
        None
    }
}

/// Full-range vs concentrated tick span (Uniswap V3 min/max ≈ ±887272).
pub(crate) fn v3_range_label(tick_lower: i32, tick_upper: i32) -> &'static str {
    if tick_lower <= -887_000 && tick_upper >= 887_000 {
        "Full range"
    } else {
        "Custom"
    }
}

/// Compact Range column.
pub(crate) fn v3_range_short(tick_lower: i32, tick_upper: i32) -> &'static str {
    if tick_lower <= -887_000 && tick_upper >= 887_000 {
        "Full"
    } else {
        "Cust"
    }
}

/// Fees waiting to collect — hide noisy `0/0`.
pub(crate) fn v3_fees_owed_label(owed0: u128, owed1: u128) -> &'static str {
    if owed0 == 0 && owed1 == 0 {
        "—"
    } else {
        "Ready"
    }
}

/// Compact Fee column (`2%` / `.05%`).
pub(crate) fn fee_tier_short(fee: u32) -> String {
    let pct = fee as f64 / 10_000.0;
    if pct >= 1.0 {
        format!("{pct:.0}%")
    } else if pct >= 0.1 {
        format!("{pct:.1}%")
    } else {
        format!(".{:02}", (pct * 100.0).round() as u32)
    }
}

/// Shorten large raw liquidity units for manage summaries.
pub(crate) fn short_units(n: alloy::primitives::U256) -> String {
    if n < alloy::primitives::U256::from(10_000u64) {
        return n.to_string();
    }
    // Scientific-ish for monospace tables: 1.98e18
    let s = n.to_string();
    let exp = s.len().saturating_sub(1);
    let lead: String = s.chars().take(3).collect();
    if lead.len() == 1 {
        format!("{lead}e{exp}")
    } else if lead.len() == 2 {
        format!("{}.{}e{exp}", &lead[..1], &lead[1..])
    } else {
        format!("{}.{}e{exp}", &lead[..1], &lead[1..])
    }
}

/// V3 column widths (Pair · NFT · Fee · [Amt0 · Amt1] · Liquidity · Range · Unclaimed).
pub(crate) struct V3TableCols {
    pub pair: usize,
    pub nft: usize,
    pub fee: usize,
    pub amt0: Option<usize>,
    pub amt1: Option<usize>,
    pub liq: usize,
    pub range: usize,
    pub unclaimed: usize,
}

/// Even split; drop Amt0/Amt1 under ~88 usable cells.
pub(crate) fn v3_table_cols(term_width: u16) -> V3TableCols {
    let mark_overhead = 2usize; // ▸ + space
    let usable = (term_width as usize).saturating_sub(mark_overhead).max(18);
    let with_amts = usable >= 88;
    let n = if with_amts { 8usize } else { 6usize };
    let gaps = n - 1;
    let content = usable.saturating_sub(gaps).max(n);
    let base = content / n;
    let rem = content % n;
    let mut widths = vec![base; n];
    for (i, w) in widths.iter_mut().enumerate() {
        if i < rem {
            *w += 1;
        }
        *w = (*w).max(3);
    }
    if with_amts {
        V3TableCols {
            pair: widths[0],
            nft: widths[1],
            fee: widths[2],
            amt0: Some(widths[3]),
            amt1: Some(widths[4]),
            liq: widths[5],
            range: widths[6],
            unclaimed: widths[7],
        }
    } else {
        V3TableCols {
            pair: widths[0],
            nft: widths[1],
            fee: widths[2],
            amt0: None,
            amt1: None,
            liq: widths[3],
            range: widths[4],
            unclaimed: widths[5],
        }
    }
}

/// V3 position table header (shared by List + manage).
pub(crate) fn v3_table_header_line(term_width: u16) -> ratatui::text::Line<'static> {
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    let c = v3_table_cols(term_width);
    let mut s = format!(
        "  {} {} {}",
        pad_col("Pair", c.pair),
        pad_col("NFT", c.nft),
        pad_col("Fee", c.fee),
    );
    if let (Some(a0), Some(a1)) = (c.amt0, c.amt1) {
        s.push(' ');
        s.push_str(&pad_col("Amt0", a0));
        s.push(' ');
        s.push_str(&pad_col("Amt1", a1));
    }
    s.push(' ');
    s.push_str(&pad_col("Liquidity", c.liq));
    s.push(' ');
    s.push_str(&pad_col("Range", c.range));
    s.push(' ');
    s.push_str(&pad_col("Unclaimed", c.unclaimed));
    Line::from(Span::styled(s, Style::default().fg(Color::DarkGray)))
}

/// One V3 row; `selected` draws ▸ + accent.
pub(crate) fn v3_table_row_line(
    chain_id: u64,
    p: &V3LpPositionView,
    assets: &[Balance],
    selected: bool,
    term_width: u16,
) -> ratatui::text::Line<'static> {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    let c = v3_table_cols(term_width);
    let mark = if selected { "▸" } else { " " };
    let pair = v3_position_pair_label(chain_id, p.token0, p.token1, assets);
    let d0 = decimals_for_token(p.token0, assets);
    let d1 = decimals_for_token(p.token1, assets);
    let amt0 = compact_token_amount(&p.amount0, d0);
    let amt1 = compact_token_amount(&p.amount1, d1);
    let mut row = format!(
        "{mark} {} {} {}",
        pad_col(&pair, c.pair),
        pad_col(&format!("#{}", p.token_id), c.nft),
        pad_col(&fee_tier_display(p.fee), c.fee),
    );
    if let (Some(a0w), Some(a1w)) = (c.amt0, c.amt1) {
        row.push(' ');
        row.push_str(&pad_col(&amt0, a0w));
        row.push(' ');
        row.push_str(&pad_col(&amt1, a1w));
    }
    row.push(' ');
    row.push_str(&pad_col(v3_liquidity_status(p.liquidity), c.liq));
    row.push(' ');
    row.push_str(&pad_col(v3_range_label(p.tick_lower, p.tick_upper), c.range));
    row.push(' ');
    row.push_str(&pad_col(
        v3_fees_owed_label(p.tokens_owed0, p.tokens_owed1),
        c.unclaimed,
    ));
    let style = if selected {
        Style::default()
            .fg(brand::accent_color())
            .add_modifier(Modifier::BOLD)
    } else if p.liquidity <= 1 {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };
    Line::from(Span::styled(row, style))
}

/// Multi-line PulseX-style V3 position detail for the Enter-focused view.
pub(crate) fn v3_focused_detail_lines(
    chain_id: u64,
    venue_label: &str,
    p: &V3LpPositionView,
    assets: &[Balance],
) -> Vec<ratatui::text::Line<'static>> {
    use alloy::primitives::U256;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use vaughan_core::core::pool_tick_to_human_price;

    let sym0 = symbol_for_token_address(chain_id, p.token0, assets);
    let sym1 = symbol_for_token_address(chain_id, p.token1, assets);
    let d0 = decimals_for_token(p.token0, assets);
    let d1 = decimals_for_token(p.token1, assets);
    let amt0 = compact_token_amount(&p.amount0, d0);
    let amt1 = compact_token_amount(&p.amount1, d1);
    let fee0 = compact_token_amount(&U256::from(p.tokens_owed0), d0);
    let fee1 = compact_token_amount(&U256::from(p.tokens_owed1), d1);
    let range_st = p.range_status().label();
    let range_kind = v3_range_label(p.tick_lower, p.tick_upper);
    let pool_s = if p.pool.is_zero() {
        "—".into()
    } else {
        short_pair_addr(p.pool)
    };

    let (unit0, unit1) = if p.sqrt_price_x96.is_zero() {
        ("—".into(), "—".into())
    } else {
        match pool_tick_to_human_price(
            chain_id,
            p.token0,
            p.token1,
            d0,
            d1,
            p.tick_current,
        ) {
            Ok(spot) => (
                format!("1={spot} {sym1}"),
                match pool_tick_to_human_price(chain_id, p.token1, p.token0, d1, d0, -p.tick_current)
                {
                    Ok(inv) => format!("1={inv} {sym0}"),
                    Err(_) => "—".into(),
                },
            ),
            Err(_) => ("—".into(), "—".into()),
        }
    };

    let muted = Style::default().fg(Color::DarkGray);
    let body = Style::default().fg(brand::body_color());
    let title = Style::default()
        .fg(brand::accent_color())
        .add_modifier(Modifier::BOLD);

    vec![
        Line::from(Span::styled(
            format!("YOUR POSITION · NFT #{} · {range_st}", p.token_id),
            title,
        )),
        Line::from(Span::styled(
            format!("  DEX   · {venue_label} V3"),
            muted,
        )),
        Line::from(Span::styled(
            format!("  Pool  · {pool_s}"),
            muted,
        )),
        Line::from(Span::styled(
            format!("  Fee   · {}", fee_tier_display(p.fee)),
            muted,
        )),
        Line::from(Span::styled(
            format!(
                "  Range · {range_kind} · ticks {}…{}",
                p.tick_lower, p.tick_upper
            ),
            muted,
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!(
                "  {:<10} {:<14} {:<18} {:<8} {}",
                "TOKEN", "AMOUNT", "UNIT PRICE (spot)", "VALUE", "FEES"
            ),
            muted,
        )),
        Line::from(Span::styled(
            format!(
                "  {:<10} {:<14} {:<18} {:<8} {}",
                trunc(&sym0, 10),
                trunc(&amt0, 14),
                trunc(&unit0, 18),
                "—",
                trunc(&fee0, 12)
            ),
            body,
        )),
        Line::from(Span::styled(
            format!(
                "  {:<10} {:<14} {:<18} {:<8} {}",
                trunc(&sym1, 10),
                trunc(&amt1, 14),
                trunc(&unit1, 18),
                "—",
                trunc(&fee1, 12)
            ),
            body,
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  Liquidity   {}", p.liquidity),
            body,
        )),
    ]
}

/// V2 column widths (Pair · Share · Amt0 · Amt1 · LP · Pair#) — even split; drop trailing on narrow.
pub(crate) struct V2TableCols {
    pub pair: usize,
    pub share: usize,
    pub amt0: usize,
    pub amt1: usize,
    pub lp: usize,
    pub addr: Option<usize>,
}

pub(crate) fn v2_table_cols(term_width: u16) -> V2TableCols {
    let mark_overhead = 2usize; // ▸ + space
    let usable = (term_width as usize).saturating_sub(mark_overhead).max(12);
    // Prefer 6 cols; drop pair-addr under ~72 cells of content.
    let with_addr = usable >= 72;
    let n = if with_addr { 6usize } else { 5usize };
    let gaps = n - 1;
    let content = usable.saturating_sub(gaps).max(n);
    let base = content / n;
    let rem = content % n;
    let mut widths = vec![base; n];
    for (i, w) in widths.iter_mut().enumerate() {
        if i < rem {
            *w += 1;
        }
        *w = (*w).max(3);
    }
    V2TableCols {
        pair: widths[0],
        share: widths[1],
        amt0: widths[2],
        amt1: widths[3],
        lp: widths[4],
        addr: with_addr.then_some(widths[5]),
    }
}

fn decimals_for_token(
    addr: alloy::primitives::Address,
    assets: &[Balance],
) -> u8 {
    let raw = format!("{addr:#x}");
    assets
        .iter()
        .find(|b| {
            b.token
                .contract_address
                .as_ref()
                .is_some_and(|a| a.eq_ignore_ascii_case(&raw))
        })
        .map(|b| b.token.decimals)
        .unwrap_or(18)
}

/// Compact human amount for table cells (e.g. `53.93B`, `1.2M`, `0.05`).
pub(crate) fn compact_token_amount(raw: &alloy::primitives::U256, decimals: u8) -> String {
    use vaughan_core::core::format_display_amount;
    let human = format_display_amount(&raw.to_string(), decimals, 6);
    compact_human_float(&human)
}

fn compact_human_float(human: &str) -> String {
    let Ok(v) = human.parse::<f64>() else {
        return human.to_string();
    };
    if !v.is_finite() {
        return human.to_string();
    }
    let abs = v.abs();
    if abs >= 1e12 {
        format!("{:.2}T", v / 1e12)
    } else if abs >= 1e9 {
        format!("{:.2}B", v / 1e9)
    } else if abs >= 1e6 {
        format!("{:.2}M", v / 1e6)
    } else if abs >= 1e3 {
        format!("{:.2}K", v / 1e3)
    } else if abs >= 1.0 {
        trim_float_string(v)
    } else if abs > 0.0 {
        format!("{v:.4}").trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        "0".into()
    }
}

pub(crate) fn format_share_pct(bps: u32) -> String {
    if bps >= 10_000 {
        "100%".into()
    } else if bps.is_multiple_of(100) {
        format!("{}%", bps / 100)
    } else {
        format!("{:.2}%", bps as f64 / 100.0)
    }
}

pub(crate) fn short_pair_addr(addr: alloy::primitives::Address) -> String {
    let raw = format!("{addr:#x}");
    if raw.len() < 12 {
        return raw;
    }
    format!("{}…{}", &raw[..6], &raw[raw.len().saturating_sub(4)..])
}

/// V2 position table header (shared by List + manage).
pub(crate) fn v2_table_header_line(term_width: u16) -> ratatui::text::Line<'static> {
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    let c = v2_table_cols(term_width);
    let mut s = format!(
        "  {} {} {} {} {}",
        pad_col("Pair", c.pair),
        pad_col("Share", c.share),
        pad_col("Token0", c.amt0),
        pad_col("Token1", c.amt1),
        pad_col("LP", c.lp),
    );
    if let Some(aw) = c.addr {
        s.push(' ');
        s.push_str(&pad_col("Pair#", aw));
    }
    Line::from(Span::styled(s, Style::default().fg(Color::DarkGray)))
}

pub(crate) fn v2_table_row_line(
    chain_id: u64,
    p: &vaughan_core::core::V2LpPosition,
    assets: &[Balance],
    selected: bool,
    term_width: u16,
) -> ratatui::text::Line<'static> {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    let c = v2_table_cols(term_width);
    let mark = if selected { "▸" } else { " " };
    let pair = v3_position_pair_label(chain_id, p.token0, p.token1, assets);
    let (a0, a1) = p.underlying_amounts();
    let d0 = decimals_for_token(p.token0, assets);
    let d1 = decimals_for_token(p.token1, assets);
    let share = format_share_pct(p.pool_share_bps());
    let amt0 = compact_token_amount(&a0, d0);
    let amt1 = compact_token_amount(&a1, d1);
    // LP tokens are almost always 18 decimals on UniV2 forks.
    let lp = compact_token_amount(&p.lp_balance, 18);
    let mut row = format!(
        "{mark} {} {} {} {} {}",
        pad_col(&pair, c.pair),
        pad_col(&share, c.share),
        pad_col(&amt0, c.amt0),
        pad_col(&amt1, c.amt1),
        pad_col(&lp, c.lp),
    );
    if let Some(aw) = c.addr {
        row.push(' ');
        row.push_str(&pad_col(&short_pair_addr(p.pair), aw));
    }
    let style = if selected {
        Style::default()
            .fg(brand::accent_color())
            .add_modifier(Modifier::BOLD)
    } else if p.lp_balance.is_zero() {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };
    Line::from(Span::styled(row, style))
}

/// Multi-line PulseX-style V2 position detail for the Enter-focused view.
pub(crate) fn v2_focused_detail_lines(
    chain_id: u64,
    venue_label: &str,
    p: &vaughan_core::core::V2LpPosition,
    assets: &[Balance],
) -> Vec<ratatui::text::Line<'static>> {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use vaughan_core::core::{format_display_amount, v2_spot_token1_per_token0};

    let sym0 = symbol_for_token_address(chain_id, p.token0, assets);
    let sym1 = symbol_for_token_address(chain_id, p.token1, assets);
    let d0 = decimals_for_token(p.token0, assets);
    let d1 = decimals_for_token(p.token1, assets);
    let (a0, a1) = p.underlying_amounts();
    let share = format_share_pct(p.pool_share_bps());
    // V2 positions are 50/50 of the underlying basket by design.
    let side_share = "50%";
    let spot = v2_spot_token1_per_token0(p.reserve0, p.reserve1, d0, d1)
        .unwrap_or_else(|| "—".into());
    // Inverse for token0 unit price in token1 terms already; token0 row shows 1 SYM0 = spot SYM1
    let unit0 = format!("1={spot} {sym1}");
    let unit1 = if let Some(inv) =
        v2_spot_token1_per_token0(p.reserve1, p.reserve0, d1, d0)
    {
        format!("1={inv} {sym0}")
    } else {
        "—".into()
    };
    let amt0 = compact_token_amount(&a0, d0);
    let amt1 = compact_token_amount(&a1, d1);
    let lp = format_display_amount(&p.lp_balance.to_string(), 18, 6);
    let res0 = compact_token_amount(&p.reserve0, d0);
    let res1 = compact_token_amount(&p.reserve1, d1);

    let muted = Style::default().fg(Color::DarkGray);
    let body = Style::default().fg(brand::body_color());
    let title = Style::default()
        .fg(brand::accent_color())
        .add_modifier(Modifier::BOLD);

    vec![
        Line::from(Span::styled(
            format!("YOUR POSITION · pool share {share}"),
            title,
        )),
        Line::from(Span::styled(
            format!("  DEX   · {venue_label} V2"),
            muted,
        )),
        Line::from(Span::styled(
            format!("  Pair  · {}", short_pair_addr(p.pair)),
            muted,
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!(
                "  {:<10} {:<14} {:<18} {:<8} {}",
                "TOKEN", "AMOUNT", "UNIT PRICE", "VALUE", "SHARE"
            ),
            muted,
        )),
        Line::from(Span::styled(
            format!(
                "  {:<10} {:<14} {:<18} {:<8} {}",
                trunc(&sym0, 10),
                trunc(&amt0, 14),
                trunc(&unit0, 18),
                "—",
                side_share
            ),
            body,
        )),
        Line::from(Span::styled(
            format!(
                "  {:<10} {:<14} {:<18} {:<8} {}",
                trunc(&sym1, 10),
                trunc(&amt1, 14),
                trunc(&unit1, 18),
                "—",
                side_share
            ),
            body,
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  Pool share     {share}"),
            body,
        )),
        Line::from(Span::styled(
            format!("  LP balance     {lp}"),
            body,
        )),
        Line::from(Span::styled(
            format!("  Total reserves {res0} {sym0} · {res1} {sym1}"),
            body,
        )),
    ]
}

fn trunc(s: &str, width: usize) -> String {
    let mut out: String = s.chars().take(width).collect();
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}

/// Pad/truncate for monospace List columns.
pub(crate) fn pad_col(s: &str, width: usize) -> String {
    let mut out = s.chars().take(width).collect::<String>();
    while out.chars().count() < width {
        out.push(' ');
    }
    out
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
