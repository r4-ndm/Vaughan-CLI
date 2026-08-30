//! Browserless LP — wiz4rd V3 on testnet 943, 9inch V3 on Pulse mainnet 369.
//!
//! **Add LP** mirrors [9inch V3 add liquidity](https://9inch.io/liquidity/add/v3?chain=pulse)
//! and [9mm V3 range UI](https://dex.9mm.pro/add/PLS/…): pair + fee, four-column
//! price range, ±% presets, then deposits.

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

/// HEX on PulseChain mainnet (8 decimals).
const HEX_MAINNET: &str = "0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";

fn format_unit_price(value: &str, sym0: &str, sym1: &str) -> String {
    format!("1 {sym0} = {value} {sym1}")
}

fn lp_tx_error_message(err: &WalletError, step: LpDeployLastStep) -> String {
    match err {
        WalletError::GasEstimationFailed(detail) => lp_gas_error_message(detail, step),
        WalletError::TransactionFailed(detail) if !detail.trim().is_empty() => {
            format!("{} · {}", err.user_message(), detail.trim())
        }
        _ => err.user_message(),
    }
}

fn lp_gas_error_message(detail: &str, step: LpDeployLastStep) -> String {
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

fn unit_price_input_line(input: &Input, quote_sym1: &str, focused: bool) -> Line<'static> {
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

fn render_unit_price_input(
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
const LP_FEE_TIERS: &[u32] = &[100, 500, 2500, 10_000, 20_000];

/// 9mm-style symmetric range shortcuts around the current price (`None` = full range).
const RANGE_PRESETS: &[(&str, Option<f64>)] = &[
    ("1%", Some(1.0)),
    ("2%", Some(2.0)),
    ("5%", Some(5.0)),
    ("10%", Some(10.0)),
    ("20%", Some(20.0)),
    ("50%", Some(50.0)),
    ("Full", None),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    List,
    AddLp,
    Increase,
    Decrease,
    Collect,
    Remove,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::AddLp => "Add LP",
            Self::Increase => "Increase",
            Self::Decrease => "Decrease",
            Self::Collect => "Collect",
            Self::Remove => "Remove",
        }
    }

    fn v3_cycle() -> &'static [Self] {
        &[
            Self::List,
            Self::AddLp,
            Self::Increase,
            Self::Decrease,
            Self::Collect,
        ]
    }

    fn v2_cycle() -> &'static [Self] {
        &[Self::List, Self::AddLp, Self::Remove]
    }

    fn next(self, stack: LpStack) -> Self {
        let tabs = match stack {
            LpStack::V3 { .. } => Self::v3_cycle(),
            LpStack::V2 { .. } => Self::v2_cycle(),
        };
        let idx = tabs.iter().position(|t| *t == self).unwrap_or(0);
        tabs[(idx + 1) % tabs.len()]
    }

    fn prev(self, stack: LpStack) -> Self {
        let tabs = match stack {
            LpStack::V3 { .. } => Self::v3_cycle(),
            LpStack::V2 { .. } => Self::v2_cycle(),
        };
        let idx = tabs.iter().position(|t| *t == self).unwrap_or(0);
        tabs[(idx + tabs.len() - 1) % tabs.len()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Input,
    Confirm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Busy {
    Idle,
    Loading,
    Sending,
}

/// V3 add flow: pair + fee, then price range + deposits (9inch / 9mm style).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AddStep {
    SelectPair,
    PriceDeposit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    None,
    Venue,
    Token0,
    Token1,
    Fee,
    /// ±% / Full range chip row (9mm-style).
    RangePresets,
    InitialPrice,
    MinPrice,
    MaxPrice,
    Ratio,
    Amount0,
    Amount1,
}

struct SortedPair {
    token0: alloy::primitives::Address,
    token1: alloy::primitives::Address,
    dec0: u8,
    dec1: u8,
    first_is_token0: bool,
}

struct V3DepositPreviewContext {
    pair: SortedPair,
    pool_min: String,
    pool_max: String,
    sqrt: alloy::primitives::U160,
    tick: i32,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum LpDeployLastStep {
    #[default]
    None,
    CreatePool,
    Initialize,
    Approve,
    AddLiquidity,
}

impl LpDeployLastStep {
    fn from_deploy_label(label: &str) -> Self {
        match label {
            "createPool" => Self::CreatePool,
            "initialize" => Self::Initialize,
            "add liquidity" => Self::AddLiquidity,
            _ if label.starts_with("approve") => Self::Approve,
            _ => Self::None,
        }
    }
}

pub struct LpView {
    stack: LpStack,
    venue: DexVenue,
    tab: Tab,
    stage: Stage,
    busy: Busy,
    tick: u64,
    status: String,
    chain_id: u64,
    v3_positions: Vec<V3PositionInfo>,
    v2_positions: Vec<V2LpPosition>,
    sel: usize,
    add_step: AddStep,
    focus: Focus,
    token0: Input,
    token1: Input,
    token0_pick: usize,
    token1_pick: usize,
    token0_editing: bool,
    token1_editing: bool,
    fee_tier: u32,
    initial_price: Input,
    min_price: Input,
    max_price: Input,
    ratio: Input,
    amount0: Input,
    amount1: Input,
    dec0: Input,
    dec1: Input,
    liquidity: Input,
    confirm_lines: Vec<Line<'static>>,
    pending_tx: Option<vaughan_core::chains::EvmTransaction>,
    /// Highlighted preset chip when [`Focus::RangePresets`].
    range_preset_idx: usize,
    /// Applied preset index, or `None` after manual min/max/current edits.
    range_preset_applied: Option<usize>,
    /// On-chain pool lifecycle for V3 deposit coupling (after [`UiJob::LpV3PoolQuote`]).
    pool_lifecycle: Option<V3PoolLifecycle>,
    pool_sqrt_x96: Option<alloy::primitives::U160>,
    pool_tick: Option<i32>,
    /// Background [`UiJob::LpV3PoolQuote`] in flight (step 2 stays interactive).
    pool_quote_inflight: bool,
    /// Preset-first range; press `a` to reveal min/current/max for fine-tuning.
    v3_custom_range: bool,
    /// Multi-step V3 deploy (createPool → initialize → approve → mint).
    lp_deploy_active: bool,
    lp_deploy_pending_resume: bool,
    lp_deploy_last_step: LpDeployLastStep,
}

impl LpView {
    pub fn for_chain(chain_id: u64) -> Self {
        let stack = lp_stack_for_chain(chain_id).unwrap_or(LpStack::V3 {
            venue: DexVenue::Wiz4rd,
        });
        let venue = stack.venue();
        let mut v = Self {
            stack,
            venue,
            tab: Tab::AddLp,
            stage: Stage::Input,
            busy: Busy::Idle,
            tick: 0,
            status: String::new(),
            chain_id,
            v3_positions: Vec::new(),
            v2_positions: Vec::new(),
            sel: 0,
            add_step: AddStep::SelectPair,
            focus: Focus::None,
            token0: Input::new(false, "select token"),
            token1: Input::new(false, "select token"),
            token0_pick: TOKEN_PICK_UNINIT,
            token1_pick: TOKEN_PICK_UNINIT,
            token0_editing: false,
            token1_editing: false,
            fee_tier: 2500,
            initial_price: Input::new(false, "0.5"),
            min_price: Input::new(false, "min · 2nd/1st"),
            max_price: Input::new(false, "max · 2nd/1st"),
            ratio: Input::new(false, "ratio · 2nd/1st"),
            amount0: Input::new(false, "0.0"),
            amount1: Input::new(false, "0.0"),
            dec0: Input::new(false, "decimals0"),
            dec1: Input::new(false, "decimals1"),
            liquidity: Input::new(false, "liquidity (raw units)"),
            confirm_lines: Vec::new(),
            pending_tx: None,
            range_preset_idx: 3,
            range_preset_applied: None,
            pool_lifecycle: None,
            pool_sqrt_x96: None,
            pool_tick: None,
            pool_quote_inflight: false,
            v3_custom_range: false,
            lp_deploy_active: false,
            lp_deploy_pending_resume: false,
            lp_deploy_last_step: LpDeployLastStep::None,
        };
        v.amount0.set_value("1");
        v.amount1.set_value("1");
        v.ratio.set_value("1");
        v.initial_price.set_value("1");
        v.dec0.set_value("18");
        v.dec1.set_value("18");
        v.apply_venue_token_defaults(true);
        v.apply_initial_fee_defaults();
        v.status = v.default_status_hint();
        v
    }

    fn lp_supported(&self) -> bool {
        match self.stack {
            LpStack::V3 { .. } => venue_position_manager(self.venue, self.chain_id).is_some(),
            LpStack::V2 { .. } => {
                venue_swap_router(self.venue, DexProtocol::V2, self.chain_id).is_some()
            }
        }
    }

    fn default_status_hint(&self) -> String {
        if self.lp_supported() {
            format!(
                "{} · {} · ←→ tab · r reload · Esc back",
                self.venue.label(),
                self.stack.label()
            )
        } else {
            format!(
                "{} {} unavailable on {} — switch network (F1)",
                self.venue.label(),
                self.stack.label(),
                chain_label(self.chain_id)
            )
        }
    }

    /// Token pair defaults when the venue changes. Does not touch fee tier (use ←→).
    fn apply_venue_token_defaults(&mut self, fill_price_hints: bool) {
        match (self.chain_id, self.venue) {
            (943, DexVenue::Wiz4rd) => {
                self.token0.set_value(WZRD_SMOKE_943);
                if let Some(wpls) = wpls_for_chain(self.chain_id) {
                    self.token1.set_value(format!("{wpls}"));
                }
                self.dec0.set_value("18");
                self.dec1.set_value("18");
            }
            (369, DexVenue::NineInch) => {
                self.token0.set_value(HEX_MAINNET);
                if let Some(wpls) = wpls_for_chain(self.chain_id) {
                    self.token1.set_value(format!("{wpls}"));
                }
                self.dec0.set_value("8");
                self.dec1.set_value("18");
            }
            (369, DexVenue::NineMm) => {
                if let Some(wpls) = wpls_for_chain(self.chain_id) {
                    self.token0.set_value(format!("{wpls}"));
                }
                self.token1
                    .set_value("0x7b39712Ef45F7dcED2bBDF11F3D5046bA61dA719");
                self.dec0.set_value("18");
                self.dec1.set_value("18");
                if fill_price_hints {
                    self.initial_price.set_value("0.001265324");
                    let (min, max) = display_price_range_from_preset(0.001_265_324, 50.0);
                    self.min_price.set_value(trim_float_string(min));
                    self.max_price.set_value(trim_float_string(max));
                    self.range_preset_idx = 5;
                    self.range_preset_applied = Some(5);
                }
            }
            _ => {}
        }
    }

    fn apply_initial_fee_defaults(&mut self) {
        match (self.chain_id, self.venue) {
            (943, DexVenue::Wiz4rd) => self.fee_tier = 500,
            (369, DexVenue::NineInch) => self.fee_tier = 2500,
            (369, DexVenue::NineMm) => self.fee_tier = 10_000,
            _ => {}
        }
    }

    fn sync_liquidity_from_selection(&mut self) {
        match self.stack {
            LpStack::V3 { .. } => {
                if let Some(p) = self.v3_positions.get(self.sel) {
                    self.liquidity.set_value(p.liquidity.to_string());
                }
            }
            LpStack::V2 { .. } => {
                if let Some(p) = self.v2_positions.get(self.sel) {
                    self.liquidity.set_value(p.lp_balance.to_string());
                }
            }
        }
    }

    fn on_tab_changed(&mut self) {
        self.focus = Focus::None;
        if self.tab == Tab::AddLp {
            self.add_step = AddStep::SelectPair;
            self.clear_pool_quote();
        }
        if matches!(self.tab, Tab::Decrease | Tab::Remove) {
            self.sync_liquidity_from_selection();
        }
    }

    fn clear_pool_quote(&mut self) {
        self.pool_lifecycle = None;
        self.pool_sqrt_x96 = None;
        self.pool_tick = None;
        self.v3_custom_range = false;
        self.pool_quote_inflight = false;
    }

    /// Assume a new pool until background RPC confirms otherwise (local preview only).
    fn begin_optimistic_pool_preview(&mut self) {
        self.pool_lifecycle = Some(V3PoolLifecycle::Missing);
        self.pool_sqrt_x96 = None;
        self.pool_tick = None;
        self.v3_custom_range = false;
        self.pool_quote_inflight = false;
    }

    fn spawn_pool_quote_job(&mut self, wallet: &WalletState) -> KeyOutcome {
        let Some(job) = self.pool_quote_job(wallet) else {
            return KeyOutcome::Consumed;
        };
        self.pool_quote_inflight = true;
        self.refresh_price_deposit_status();
        KeyOutcome::StartJob(job)
    }

    fn pool_quote_job(&self, wallet: &WalletState) -> Option<UiJob> {
        if !matches!(self.stack, LpStack::V3 { .. }) {
            return None;
        }
        let pair = self.sorted_pair().ok()?;
        Some(UiJob::LpV3PoolQuote {
            venue: self.venue,
            chain_id: self.chain_id,
            rpc_url: wallet.active_rpc_url(),
            token0: format!("{:#x}", pair.token0),
            token1: format!("{:#x}", pair.token1),
            fee: self.fee_tier,
            dec0: pair.dec0,
            dec1: pair.dec1,
        })
    }

    fn apply_pool_quote(&mut self, quote: vaughan_core::core::V3LpPoolQuote) {
        self.pool_quote_inflight = false;
        if self.add_step != AddStep::PriceDeposit {
            return;
        }
        self.pool_lifecycle = Some(quote.lifecycle);
        self.pool_sqrt_x96 = quote.sqrt_price_x96;
        self.pool_tick = quote.tick;
        if quote.lifecycle == V3PoolLifecycle::Ready {
            if let Some(pool_price) = quote.pool_price_token1_per_token0 {
                if let Ok(pair) = self.sorted_pair() {
                    let user = self.pool_price_to_user_price(pair.first_is_token0, &pool_price);
                    self.initial_price.set_value(user);
                }
            }
        }
        self.resync_range_bounds_from_preset();
        self.sync_amount1_from_price();
        self.normalize_v3_price_focus();
        if self.range_preset_applied.is_some() {
            self.focus_v3_deposit_after_preset();
        }
        self.refresh_price_deposit_status();
    }

    fn needs_v3_starting_price(&self) -> bool {
        matches!(
            self.pool_lifecycle,
            Some(V3PoolLifecycle::Missing) | Some(V3PoolLifecycle::Uninitialized { .. }) | None
        )
    }

    fn on_v3_price_deposit(&self) -> bool {
        matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::PriceDeposit
    }

    /// Keep keyboard focus on a visible field (pool quote / custom-range toggles hide inputs).
    fn normalize_v3_price_focus(&mut self) {
        if !self.on_v3_price_deposit() {
            return;
        }
        match self.focus {
            Focus::MinPrice | Focus::MaxPrice if !self.v3_custom_range => {
                self.focus = Focus::RangePresets;
            }
            Focus::InitialPrice
                if !self.needs_v3_starting_price()
                    || (!self.v3_custom_range
                        && self.pool_lifecycle == Some(V3PoolLifecycle::Ready)) =>
            {
                self.focus = Focus::Amount0;
            }
            _ => {}
        }
    }

    fn focus_v3_deposit_after_preset(&mut self) {
        if !self.on_v3_price_deposit() {
            return;
        }
        self.normalize_v3_price_focus();
        self.focus = self.next_v3_focus_after_presets();
    }

    fn next_v3_focus_after_presets(&self) -> Focus {
        if self.v3_custom_range {
            Focus::MinPrice
        } else if self.needs_v3_starting_price() {
            Focus::InitialPrice
        } else {
            Focus::Amount0
        }
    }

    fn toggle_v3_custom_range(&mut self) {
        self.v3_custom_range = !self.v3_custom_range;
        self.status = if self.v3_custom_range {
            "Fine-tune min / current / max · a = back to presets only".into()
        } else {
            "Preset range · a = adjust min/max prices later".into()
        };
        if !self.v3_custom_range
            && (matches!(self.focus, Focus::MinPrice | Focus::MaxPrice)
                || (self.focus == Focus::InitialPrice && !self.needs_v3_starting_price()))
        {
            self.focus = Focus::RangePresets;
        }
        self.normalize_v3_price_focus();
    }

    /// Plain-language hint for which deposit field(s) matter at the current price + range.
    fn v3_deposit_guidance(&self, sym0: &str, sym1: &str) -> String {
        let pair = match self.sorted_pair() {
            Ok(p) => p,
            Err(_) => return format!("Enter how much {sym0} to deposit"),
        };
        let pool_initial =
            match self.user_price_to_pool_price(pair.first_is_token0, self.initial_price.value()) {
                Ok(s) if !s.trim().is_empty() => s,
                _ => return format!("Set starting price, then enter {sym0} amount"),
            };
        let pool_min = self
            .user_price_to_pool_price(pair.first_is_token0, self.min_price.value())
            .unwrap_or_default();
        let pool_max = self
            .user_price_to_pool_price(pair.first_is_token0, self.max_price.value())
            .unwrap_or_default();
        let (sqrt, tick) = match v3_sqrt_and_tick_for_preview(
            self.chain_id,
            pair.token0,
            pair.token1,
            pair.dec0,
            pair.dec1,
            self.fee_tier,
            self.pool_sqrt_x96,
            self.pool_tick,
            &pool_initial,
        ) {
            Ok(v) => v,
            Err(_) => {
                return format!("Enter {sym0} — {sym1} is calculated automatically");
            }
        };
        let (tick_lower, tick_upper) =
            match if pool_min.trim().is_empty() || pool_max.trim().is_empty() {
                default_full_range_ticks(self.fee_tier)
            } else {
                v3_range_ticks_from_human_prices(
                    self.chain_id,
                    pair.token0,
                    pair.token1,
                    pair.dec0,
                    pair.dec1,
                    pool_min.trim(),
                    pool_max.trim(),
                    self.fee_tier,
                )
            } {
                Ok(t) => t,
                Err(_) => {
                    return format!("Enter {sym0} — {sym1} is calculated automatically");
                }
            };
        let _ = sqrt;
        let (ui_primary, ui_auto, pool_token0_sym, pool_token1_sym) = if pair.first_is_token0 {
            (sym0, sym1, sym0, sym1)
        } else {
            (sym0, sym1, sym1, sym0)
        };
        if tick < tick_lower {
            let only = pool_token0_sym;
            if only == ui_primary {
                format!("Price is below your range — only {only} is needed")
            } else {
                format!(
                    "Price is below your range — only {only} is needed (widen preset or a to adjust)"
                )
            }
        } else if tick >= tick_upper {
            let only = pool_token1_sym;
            if only == ui_primary {
                format!("Price is above your range — only {only} is needed")
            } else {
                format!(
                    "Price is above your range — only {only} is needed (widen preset or a to adjust)"
                )
            }
        } else {
            format!("Enter {ui_primary} — {ui_auto} fills in automatically")
        }
    }

    fn simple_range_explainer(&self) -> &'static str {
        "Earn swap fees while the price stays inside your range · wider = safer · narrower = higher yield"
    }

    fn render_simple_range_summary(&self, frame: &mut Frame, area: Rect, sym0: &str, sym1: &str) {
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(" Your range ")));
        let current = self.initial_price.value().trim();
        let current_line = if current.is_empty() {
            Line::from(Span::styled(
                format!("Current: set 1 {sym0} = … {sym1} below"),
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            Line::from(vec![
                Span::styled("Current ", Style::default().fg(Color::DarkGray)),
                Span::raw(format_unit_price(current, sym0, sym1)),
            ])
        };
        let band = self.range_band_label();
        let band_line = Line::from(vec![
            Span::styled("Width ", Style::default().fg(Color::DarkGray)),
            Span::raw(band),
        ]);
        let fee_line = Line::from(Span::styled(
            self.v3_deposit_guidance(sym0, sym1),
            Style::default().fg(brand::body_color()),
        ));
        frame.render_widget(
            Paragraph::new(vec![current_line, band_line, fee_line]).alignment(Alignment::Center),
            inner,
        );
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        if self.stage != Stage::Input {
            return false;
        }
        if self.busy == Busy::Sending {
            return false;
        }
        if self.busy == Busy::Loading && !self.on_v3_price_deposit() {
            return false;
        }
        if self.tab != Tab::AddLp {
            return true;
        }
        match self.focus {
            Focus::None | Focus::Fee | Focus::Venue | Focus::RangePresets => true,
            Focus::Token0 => !self.token0_editing,
            Focus::Token1 => !self.token1_editing,
            Focus::InitialPrice
            | Focus::MinPrice
            | Focus::MaxPrice
            | Focus::Ratio
            | Focus::Amount0
            | Focus::Amount1 => false,
        }
    }

    /// ↑/↓ on token fields cycles wallet assets (F2 list).
    pub fn cycle_focused_token_picker(&mut self, assets: &[Balance], forward: bool) -> bool {
        if self.tab != Tab::AddLp || self.stage != Stage::Input {
            return false;
        }
        if matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::PriceDeposit {
            return false;
        }
        let mut dummy = false;
        match self.focus {
            Focus::Token0 => {
                self.token0_editing = false;
                let changed = cycle_token_picker(
                    assets,
                    true,
                    &mut self.token0_pick,
                    forward,
                    &mut dummy,
                    &mut self.token0,
                    &mut self.status,
                );
                if changed {
                    let addr = self.token0.value().to_string();
                    Self::sync_decimals_for_token(&addr, &mut self.dec0, assets);
                }
                changed
            }
            Focus::Token1 => {
                self.token1_editing = false;
                let changed = cycle_token_picker(
                    assets,
                    true,
                    &mut self.token1_pick,
                    forward,
                    &mut dummy,
                    &mut self.token1,
                    &mut self.status,
                );
                if changed {
                    let addr = self.token1.value().to_string();
                    Self::sync_decimals_for_token(&addr, &mut self.dec1, assets);
                }
                changed
            }
            _ => false,
        }
    }

    fn sync_decimals_for_token(addr: &str, dec: &mut Input, assets: &[Balance]) {
        let want = addr.trim();
        if want.is_empty() {
            return;
        }
        if let Some(b) = assets.iter().find(|b| {
            b.token
                .contract_address
                .as_ref()
                .is_some_and(|a| a.eq_ignore_ascii_case(want))
        }) {
            dec.set_value(b.token.decimals.to_string());
        }
    }

    pub fn initial_job(&self, wallet: &WalletState) -> Option<UiJob> {
        self.list_job(wallet)
    }

    fn list_job(&self, wallet: &WalletState) -> Option<UiJob> {
        if !self.lp_supported() {
            return None;
        }
        let owner = wallet.active_address().ok()?.to_string();
        let rpc = wallet.active_rpc_url();
        match self.stack {
            LpStack::V3 { .. } => Some(UiJob::LpListPositions {
                venue: self.venue,
                chain_id: self.chain_id,
                rpc_url: rpc,
                owner,
            }),
            LpStack::V2 { venue } => Some(UiJob::LpListV2Positions {
                venue,
                chain_id: self.chain_id,
                rpc_url: rpc,
                owner,
            }),
        }
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::LpPositions(Ok(rows)) => {
                self.busy = Busy::Idle;
                self.v3_positions = rows;
                if self.sel >= self.v3_positions.len() && !self.v3_positions.is_empty() {
                    self.sel = 0;
                }
                self.sync_liquidity_from_selection();
                self.status = format!(
                    "{} · {} V3 position(s)",
                    self.venue.label(),
                    self.v3_positions.len()
                );
            }
            UiJobResult::LpPositions(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = e.user_message();
            }
            UiJobResult::LpV2Positions(Ok(rows)) => {
                self.busy = Busy::Idle;
                self.v2_positions = rows;
                if self.sel >= self.v2_positions.len() && !self.v2_positions.is_empty() {
                    self.sel = 0;
                }
                self.sync_liquidity_from_selection();
                self.status = format!(
                    "{} · {} V2 position(s)",
                    self.venue.label(),
                    self.v2_positions.len()
                );
            }
            UiJobResult::LpV2Positions(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = e.user_message();
            }
            UiJobResult::LpV3PoolDeployStep(Ok((tx, label))) => {
                self.busy = Busy::Idle;
                self.pending_tx = Some(tx);
                self.lp_deploy_last_step = LpDeployLastStep::from_deploy_label(&label);
                self.confirm_lines = vec![Line::from(format!(
                    "Confirm {} {} (Enter send · Esc cancel)",
                    self.venue.label(),
                    label
                ))];
                self.status = format!("Ready — confirm {label} (Enter send · Esc cancel)");
                self.stage = Stage::Confirm;
            }
            UiJobResult::LpV3PoolDeployStep(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = lp_tx_error_message(&e, self.lp_deploy_last_step);
            }
            UiJobResult::LpV3PoolQuote(Ok(quote)) => {
                self.apply_pool_quote(quote);
            }
            UiJobResult::LpV3PoolQuote(Err(e)) => {
                if self.add_step != AddStep::PriceDeposit {
                    self.pool_quote_inflight = false;
                    return;
                }
                self.pool_quote_inflight = false;
                self.pool_lifecycle = Some(V3PoolLifecycle::Missing);
                self.status = format!(
                    "Pool lookup failed — {} · using your prices (rechecks before send)",
                    e.user_message()
                );
                self.resync_range_bounds_from_preset();
                self.sync_amount1_from_price();
            }
            UiJobResult::Send(Ok(receipt)) => {
                self.busy = Busy::Idle;
                self.pending_tx = None;
                if self.lp_deploy_active {
                    if self.lp_deploy_last_step == LpDeployLastStep::AddLiquidity {
                        self.stage = Stage::Input;
                        self.confirm_lines.clear();
                        self.lp_deploy_active = false;
                        self.lp_deploy_pending_resume = false;
                        self.lp_deploy_last_step = LpDeployLastStep::None;
                        self.status = format!("LP added ({})", receipt.hash);
                    } else {
                        self.lp_deploy_pending_resume = true;
                        self.stage = Stage::Confirm;
                        self.confirm_lines = vec![Line::from(format!(
                            "Confirmed ({}) — preparing next LP step…",
                            receipt.hash
                        ))];
                        self.status = "Preparing next LP step…".into();
                    }
                } else {
                    self.stage = Stage::Input;
                    self.confirm_lines.clear();
                    self.status = format!("LP tx ok ({})", receipt.hash);
                }
            }
            UiJobResult::Send(Err(e)) => {
                self.busy = Busy::Idle;
                self.stage = Stage::Input;
                self.pending_tx = None;
                self.status = lp_tx_error_message(&e, self.lp_deploy_last_step);
            }
            _ => {}
        }
    }

    /// After a deploy-step broadcast, queue the next create / initialize / approve / mint step.
    pub fn followup_job(&mut self, wallet: &WalletState) -> Option<UiJob> {
        if !self.lp_deploy_pending_resume {
            return None;
        }
        self.lp_deploy_pending_resume = false;
        let deploy_wait = match self.lp_deploy_last_step {
            LpDeployLastStep::CreatePool => V3LpDeployWait::AfterCreatePool,
            LpDeployLastStep::Initialize => V3LpDeployWait::AfterInitialize,
            LpDeployLastStep::Approve => V3LpDeployWait::AfterApprove,
            _ => V3LpDeployWait::None,
        };
        match self.build_lp_deploy_job(wallet, deploy_wait) {
            Ok(job) => {
                self.busy = Busy::Loading;
                self.status = match deploy_wait {
                    V3LpDeployWait::AfterCreatePool => "Waiting for createPool on chain…".into(),
                    V3LpDeployWait::AfterInitialize => {
                        "Waiting for pool initialize on chain…".into()
                    }
                    V3LpDeployWait::AfterApprove => "Waiting for approve on chain…".into(),
                    V3LpDeployWait::None => "Preparing next LP step…".into(),
                };
                Some(job)
            }
            Err(e) => {
                self.busy = Busy::Idle;
                self.stage = Stage::Input;
                self.confirm_lines.clear();
                self.lp_deploy_active = false;
                self.status = e;
                None
            }
        }
    }

    fn build_lp_deploy_job(
        &self,
        wallet: &WalletState,
        deploy_wait: V3LpDeployWait,
    ) -> Result<UiJob, String> {
        let pair = self.sorted_pair()?;
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let pool_initial =
            self.user_price_to_pool_price(pair.first_is_token0, self.initial_price.value())?;
        let pool_min =
            self.user_price_to_pool_price(pair.first_is_token0, self.min_price.value())?;
        let pool_max =
            self.user_price_to_pool_price(pair.first_is_token0, self.max_price.value())?;
        let (amount0, amount1) = if pair.first_is_token0 {
            (
                self.amount0.value().to_string(),
                self.amount1.value().to_string(),
            )
        } else {
            (
                self.amount1.value().to_string(),
                self.amount0.value().to_string(),
            )
        };
        if amount0.trim().is_empty() || amount1.trim().is_empty() {
            return Err("Enter both deposit amounts".into());
        }
        Ok(UiJob::LpV3PoolDeployStep {
            venue: self.venue,
            chain_id: self.chain_id,
            rpc_url: wallet.active_rpc_url(),
            from,
            token0: format!("{:#x}", pair.token0),
            token1: format!("{:#x}", pair.token1),
            fee: self.fee_tier,
            dec0: pair.dec0,
            dec1: pair.dec1,
            pool_initial_price: pool_initial,
            pool_min_price: pool_min,
            pool_max_price: pool_max,
            amount0,
            amount1,
            deploy_wait,
        })
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState, assets: &[Balance]) {
        let [content, status_area] = body_areas(area);
        if self.stage == Stage::Confirm {
            frame.render_widget(
                Paragraph::new(self.confirm_lines.clone())
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(brand::body_color())),
                content,
            );
        } else if self.tab == Tab::AddLp {
            self.render_add_lp(frame, content, assets);
        } else {
            let lines = self.body_lines();
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(brand::body_color())),
                content,
            );
        }
        let status = if self.busy != Busy::Idle {
            format!("{} {}", spinner_frame(self.tick), self.status)
        } else {
            self.status.clone()
        };
        frame.render_widget(status_paragraph(&status), status_area);
    }

    fn render_add_lp(&self, frame: &mut Frame, area: Rect, assets: &[Balance]) {
        let on_price_deposit =
            matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::PriceDeposit;
        let show_fee = matches!(self.stack, LpStack::V3 { .. }) && !on_price_deposit;
        let sym0 = self.token_symbol(&self.token0, assets);
        let sym1 = self.token_symbol(&self.token1, assets);
        let price_suffix = format!(" · {sym1}/{sym0}");

        let mut constraints = vec![
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ];
        if matches!(self.stack, LpStack::V3 { .. }) && !on_price_deposit {
            constraints.push(Constraint::Length(1));
        }
        if !on_price_deposit {
            constraints.push(Constraint::Length(3));
            constraints.push(Constraint::Length(1));
            constraints.push(Constraint::Length(3));
        } else {
            constraints.push(Constraint::Length(1)); // pair · fee summary
            constraints.push(Constraint::Length(1)); // explainer
            constraints.push(Constraint::Length(3)); // range preset box
            if self.v3_custom_range {
                constraints.push(Constraint::Length(4)); // min | current | max | band
            }
            if self.needs_v3_starting_price() && !self.v3_custom_range {
                constraints.push(Constraint::Length(3)); // starting price (new pool)
            }
            constraints.push(Constraint::Length(5)); // summary box (border + 3 lines)
            if !self.v3_custom_range {
                constraints.push(Constraint::Length(1)); // a = adjust hint
            }
            constraints.push(Constraint::Length(1)); // deposit title
            constraints.push(Constraint::Length(3)); // deposit row
        }
        if show_fee {
            constraints.push(Constraint::Length(1));
        }
        if on_price_deposit {
            // rows accounted above
        } else if matches!(self.stack, LpStack::V2 { .. }) {
            constraints.extend([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
            ]);
        }
        constraints.push(Constraint::Length(1));
        constraints.push(Constraint::Min(0));

        let chunks = Layout::vertical(constraints).split(area);
        let mut i = 0;

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " Add liquidity ",
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD),
            )))
            .alignment(Alignment::Center),
            chunks[i],
        );
        i += 1;

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Tab ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!("{}   (←→)", self.tab_bar())),
            ]))
            .alignment(Alignment::Center),
            chunks[i],
        );
        i += 1;

        let step_label = if on_price_deposit {
            "Step 2/2 — Pick a range preset (a to fine-tune) · then deposit"
        } else if matches!(self.stack, LpStack::V3 { .. }) {
            "Step 1/2 — Pick the two tokens and fee tier"
        } else {
            "Select tokens, ratio, and deposit"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                step_label,
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Center),
            chunks[i],
        );
        i += 1;

        if matches!(self.stack, LpStack::V3 { .. }) && !on_price_deposit {
            let venue_style = if self.focus == Focus::Venue {
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(brand::body_color())
            };
            let picker: Vec<_> = lp_v3_venue_picker(self.chain_id)
                .iter()
                .map(|v| {
                    let on_chain = venue_position_manager(*v, self.chain_id).is_some();
                    let mark = if *v == self.venue { "[" } else { "" };
                    let end = if *v == self.venue { "]" } else { "" };
                    if on_chain {
                        format!("{mark}{}{end}", v.label())
                    } else {
                        format!("{mark}{}(943){end}", v.label())
                    }
                })
                .collect();
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("{} · ↑↓ pick", picker.join(" · ")),
                    venue_style,
                )))
                .alignment(Alignment::Center),
                chunks[i],
            );
            i += 1;
        }

        if !on_price_deposit {
            self.render_token_field(
                frame,
                chunks[i],
                "First token",
                &self.token0,
                self.focus == Focus::Token0,
                assets,
                self.token0_editing,
                area.width,
            );
            i += 1;
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "+",
                    Style::default().fg(Color::DarkGray),
                )))
                .alignment(Alignment::Center),
                chunks[i],
            );
            i += 1;
            self.render_token_field(
                frame,
                chunks[i],
                "Second token",
                &self.token1,
                self.focus == Focus::Token1,
                assets,
                self.token1_editing,
                area.width,
            );
            i += 1;
        } else {
            frame.render_widget(
                Paragraph::new(Line::from(format!(
                    "{} · {} + {} · fee {}",
                    self.venue.label(),
                    sym0,
                    sym1,
                    fee_tier_display(self.fee_tier)
                )))
                .alignment(Alignment::Center),
                chunks[i],
            );
            i += 1;
        }

        if show_fee {
            frame.render_widget(
                Paragraph::new(self.fee_tier_line()).alignment(Alignment::Center),
                chunks[i],
            );
            i += 1;
        }

        if on_price_deposit {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    self.simple_range_explainer(),
                    Style::default().fg(Color::DarkGray),
                )))
                .wrap(Wrap { trim: true })
                .alignment(Alignment::Center),
                chunks[i],
            );
            i += 1;
            self.render_range_preset_row(frame, chunks[i]);
            i += 1;
            if self.v3_custom_range {
                self.render_price_range_columns(frame, chunks[i], sym0, sym1, &price_suffix);
                i += 1;
            }
            if self.needs_v3_starting_price() && !self.v3_custom_range {
                render_unit_price_input(
                    frame,
                    chunks[i],
                    &format!("Starting price (new pool) · 1 {sym0} ="),
                    sym1,
                    &self.initial_price,
                    self.focus == Focus::InitialPrice,
                    Alignment::Left,
                );
                i += 1;
            }
            self.render_simple_range_summary(frame, chunks[i], sym0, sym1);
            i += 1;
            if !self.v3_custom_range {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "a = fine-tune min / current / max later",
                        Style::default().fg(Color::DarkGray),
                    )))
                    .alignment(Alignment::Center),
                    chunks[i],
                );
                i += 1;
            }
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "How much to add?",
                    Style::default()
                        .fg(brand::accent_color())
                        .add_modifier(Modifier::BOLD),
                )))
                .alignment(Alignment::Center),
                chunks[i],
            );
            i += 1;
            self.render_deposit_columns(frame, chunks[i], sym0, sym1);
            i += 1;
        } else if matches!(self.stack, LpStack::V2 { .. }) {
            render_labeled_input(
                frame,
                chunks[i],
                &format!("Ratio{price_suffix}"),
                &self.ratio,
                self.focus == Focus::Ratio,
            );
            i += 1;
            render_labeled_input(
                frame,
                chunks[i],
                &format!("Deposit {sym0}"),
                &self.amount0,
                self.focus == Focus::Amount0,
            );
            i += 1;
            render_labeled_input(
                frame,
                chunks[i],
                &format!("Deposit {sym1}"),
                &self.amount1,
                self.focus == Focus::Amount1,
            );
            i += 1;
        }

        let hint = if on_price_deposit {
            if self.v3_custom_range {
                "Tab fields · ←→ range · Enter apply · a presets-only · Esc back"
            } else {
                "Tab fields · ←→ range · Enter apply · a fine-tune · Esc back"
            }
        } else if matches!(self.stack, LpStack::V3 { .. }) {
            "Tab · ↑↓ tokens/venue · ←→ fee · Enter continue"
        } else {
            "Tab · field · ↑↓ token · Enter · add liquidity"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hint,
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Center),
            chunks[i],
        );
    }

    fn fee_tier_line(&self) -> Line<'static> {
        let mut spans = vec![Span::raw("Fee tier: ")];
        for &tier in LP_FEE_TIERS {
            let label = fee_tier_display(tier);
            let selected = tier == self.fee_tier;
            let style = if selected {
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else if self.focus == Focus::Fee {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Span::styled(format!(" {label} "), style));
        }
        if self.focus == Focus::Fee {
            spans.push(Span::styled(
                " ←→",
                Style::default().fg(brand::accent_color()),
            ));
        }
        Line::from(spans)
    }

    fn render_range_preset_row(&self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::RangePresets;
        let title = if focused {
            brand::focus_title(" Range width ")
        } else {
            brand::fade_line(" Range width ")
        };
        let inner = brand::render_labeled_input_box(frame, area, Some(title), focused);
        frame.render_widget(
            Paragraph::new(self.range_preset_line()).alignment(Alignment::Center),
            inner,
        );
    }

    fn range_preset_line(&self) -> Line<'static> {
        let mut spans = Vec::with_capacity(RANGE_PRESETS.len() * 2);
        for (i, (label, _)) in RANGE_PRESETS.iter().enumerate() {
            let applied = self.range_preset_applied == Some(i);
            let highlighted = self.focus == Focus::RangePresets && self.range_preset_idx == i;
            let style = if applied {
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else if highlighted {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Span::styled(format!(" {label} "), style));
        }
        if self.focus == Focus::RangePresets {
            spans.push(Span::styled(
                " ←→ pick · Enter next",
                Style::default().fg(brand::accent_color()),
            ));
        }
        Line::from(spans)
    }

    fn render_price_range_columns(
        &self,
        frame: &mut Frame,
        area: Rect,
        sym0: &str,
        sym1: &str,
        price_suffix: &str,
    ) {
        let cols = Layout::horizontal([
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
        ])
        .split(area);
        render_labeled_input_aligned(
            frame,
            cols[0],
            &format!("Min{price_suffix}"),
            &self.min_price,
            self.focus == Focus::MinPrice,
            Alignment::Center,
        );
        render_unit_price_input(
            frame,
            cols[1],
            &format!("Current · 1 {sym0} ="),
            sym1,
            &self.initial_price,
            self.focus == Focus::InitialPrice,
            Alignment::Center,
        );
        render_labeled_input_aligned(
            frame,
            cols[2],
            &format!("Max{price_suffix}"),
            &self.max_price,
            self.focus == Focus::MaxPrice,
            Alignment::Center,
        );
        self.render_range_summary_cell(frame, cols[3], sym0, sym1);
    }

    fn render_range_summary_cell(&self, frame: &mut Frame, area: Rect, sym0: &str, sym1: &str) {
        let title = brand::fade_line(" Range ");
        let inner = brand::render_faded_box(frame, area, Some(title));
        let inv = self
            .center_price_f64()
            .map(|p| trim_float_string(1.0 / p))
            .unwrap_or_else(|| "—".to_string());
        let band = self.range_band_label();
        let lines = vec![
            Line::from(vec![
                Span::styled("Inv ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{inv} {sym0}/{sym1}")),
            ]),
            Line::from(Span::styled(band, Style::default().fg(brand::body_color()))),
        ];
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
    }

    fn render_deposit_columns(&self, frame: &mut Frame, area: Rect, sym0: &str, sym1: &str) {
        let cols =
            Layout::horizontal([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)]).split(area);
        render_labeled_input(
            frame,
            cols[0],
            &format!("Deposit {sym0}"),
            &self.amount0,
            self.focus == Focus::Amount0,
        );
        render_labeled_input(
            frame,
            cols[1],
            &format!("Deposit {sym1}"),
            &self.amount1,
            self.focus == Focus::Amount1,
        );
    }

    fn center_price_f64(&self) -> Option<f64> {
        parse_price_f64(self.initial_price.value()).ok()
    }

    fn range_band_label(&self) -> String {
        let min_raw = self.min_price.value().trim();
        let max_raw = self.max_price.value().trim();
        if min_raw.is_empty() && max_raw.is_empty() {
            return "Full range".into();
        }
        let Some(center) = self.center_price_f64() else {
            return "Set current price".into();
        };
        if center <= 0.0 {
            return "—".into();
        }
        let mut parts = Vec::new();
        if let Ok(min) = parse_price_f64(min_raw) {
            let lo = (1.0 - min / center) * 100.0;
            if lo.is_finite() {
                parts.push(format!("-{lo:.1}%"));
            }
        }
        if let Ok(max) = parse_price_f64(max_raw) {
            let hi = (max / center - 1.0) * 100.0;
            if hi.is_finite() {
                parts.push(format!("+{hi:.1}%"));
            }
        }
        if parts.is_empty() {
            "Custom range".into()
        } else {
            parts.join(" / ")
        }
    }

    fn cycle_range_preset_highlight(&mut self, forward: bool) {
        let n = RANGE_PRESETS.len();
        self.range_preset_idx = if forward {
            (self.range_preset_idx + 1) % n
        } else {
            (self.range_preset_idx + n - 1) % n
        };
    }

    fn apply_range_preset(&mut self, idx: usize) {
        if idx >= RANGE_PRESETS.len() {
            return;
        }
        self.range_preset_idx = idx;
        self.range_preset_applied = Some(idx);
        let (_, pct) = RANGE_PRESETS[idx];
        match pct {
            None => {
                self.min_price.set_value("");
                self.max_price.set_value("");
            }
            Some(p) => {
                let center = self.center_price_f64().unwrap_or(1.0);
                let (min, max) = display_price_range_from_preset(center, p);
                self.min_price.set_value(trim_float_string(min));
                self.max_price.set_value(trim_float_string(max));
                if self.initial_price.value().trim().is_empty() {
                    self.initial_price.set_value(trim_float_string(center));
                }
                self.sync_amount1_from_price();
            }
        }
        self.refresh_price_deposit_status();
        self.focus_v3_deposit_after_preset();
    }

    fn sync_amount1_from_price(&mut self) {
        let Some(ctx) = self.v3_deposit_preview_context() else {
            return;
        };
        let V3DepositPreviewContext {
            pair,
            pool_min,
            pool_max,
            sqrt,
            tick,
        } = ctx;

        if pair.first_is_token0 {
            let amount0_wei = match parse_swap_amount(self.amount0.value(), "amount0", pair.dec0) {
                Ok(w) => w,
                Err(_) => return,
            };
            if amount0_wei.is_zero() {
                return;
            }
            match v3_preview_mint_deposits_from_amount0(
                self.chain_id,
                pair.token0,
                pair.token1,
                pair.dec0,
                pair.dec1,
                self.fee_tier,
                sqrt,
                tick,
                &pool_min,
                &pool_max,
                amount0_wei,
            ) {
                Ok((_a0, a1)) => {
                    self.amount1.set_value(format_display_amount(
                        &a1.to_string(),
                        pair.dec1,
                        SWAP_DISPLAY_FRAC,
                    ));
                }
                Err(_) => self.sync_amount1_simple(),
            }
        } else {
            let amount1_wei = match parse_swap_amount(self.amount0.value(), "amount0", pair.dec1) {
                Ok(w) => w,
                Err(_) => return,
            };
            if amount1_wei.is_zero() {
                return;
            }
            match v3_preview_mint_deposits_from_amount1(
                self.chain_id,
                pair.token0,
                pair.token1,
                pair.dec0,
                pair.dec1,
                self.fee_tier,
                sqrt,
                tick,
                &pool_min,
                &pool_max,
                amount1_wei,
            ) {
                Ok((a0, _a1)) => {
                    self.amount1.set_value(format_display_amount(
                        &a0.to_string(),
                        pair.dec0,
                        SWAP_DISPLAY_FRAC,
                    ));
                }
                Err(_) => self.sync_amount1_simple(),
            }
        }
    }

    fn sync_amount0_from_price(&mut self) {
        let Some(ctx) = self.v3_deposit_preview_context() else {
            return;
        };
        let V3DepositPreviewContext {
            pair,
            pool_min,
            pool_max,
            sqrt,
            tick,
        } = ctx;

        if pair.first_is_token0 {
            let amount1_wei = match parse_swap_amount(self.amount1.value(), "amount1", pair.dec1) {
                Ok(w) => w,
                Err(_) => return,
            };
            if amount1_wei.is_zero() {
                return;
            }
            match v3_preview_mint_deposits_from_amount1(
                self.chain_id,
                pair.token0,
                pair.token1,
                pair.dec0,
                pair.dec1,
                self.fee_tier,
                sqrt,
                tick,
                &pool_min,
                &pool_max,
                amount1_wei,
            ) {
                Ok((a0, _a1)) => {
                    self.amount0.set_value(format_display_amount(
                        &a0.to_string(),
                        pair.dec0,
                        SWAP_DISPLAY_FRAC,
                    ));
                }
                Err(_) => self.sync_amount0_simple(),
            }
        } else {
            let amount0_wei = match parse_swap_amount(self.amount1.value(), "amount1", pair.dec0) {
                Ok(w) => w,
                Err(_) => return,
            };
            if amount0_wei.is_zero() {
                return;
            }
            match v3_preview_mint_deposits_from_amount0(
                self.chain_id,
                pair.token0,
                pair.token1,
                pair.dec0,
                pair.dec1,
                self.fee_tier,
                sqrt,
                tick,
                &pool_min,
                &pool_max,
                amount0_wei,
            ) {
                Ok((_a0, a1)) => {
                    self.amount0.set_value(format_display_amount(
                        &a1.to_string(),
                        pair.dec1,
                        SWAP_DISPLAY_FRAC,
                    ));
                }
                Err(_) => self.sync_amount0_simple(),
            }
        }
    }

    fn v3_deposit_preview_context(&self) -> Option<V3DepositPreviewContext> {
        let pair = self.sorted_pair().ok()?;
        let pool_initial = self
            .user_price_to_pool_price(pair.first_is_token0, self.initial_price.value())
            .ok()?;
        if pool_initial.trim().is_empty() {
            return None;
        }
        let pool_min = self
            .user_price_to_pool_price(pair.first_is_token0, self.min_price.value())
            .unwrap_or_default();
        let pool_max = self
            .user_price_to_pool_price(pair.first_is_token0, self.max_price.value())
            .unwrap_or_default();
        let (sqrt, tick) = v3_sqrt_and_tick_for_preview(
            self.chain_id,
            pair.token0,
            pair.token1,
            pair.dec0,
            pair.dec1,
            self.fee_tier,
            self.pool_sqrt_x96,
            self.pool_tick,
            &pool_initial,
        )
        .ok()?;
        Some(V3DepositPreviewContext {
            pair,
            pool_min,
            pool_max,
            sqrt,
            tick,
        })
    }

    fn sync_amount1_simple(&mut self) {
        let Ok(a0) = self.amount0.value().trim().parse::<f64>() else {
            return;
        };
        let Some(p) = self.center_price_f64() else {
            return;
        };
        if a0 <= 0.0 || p <= 0.0 {
            return;
        }
        self.amount1.set_value(trim_float_string(a0 * p));
    }

    fn sync_amount0_simple(&mut self) {
        let Ok(a1) = self.amount1.value().trim().parse::<f64>() else {
            return;
        };
        let Some(p) = self.center_price_f64() else {
            return;
        };
        if a1 <= 0.0 || p <= 0.0 {
            return;
        }
        self.amount0.set_value(trim_float_string(a1 / p));
    }

    fn refresh_price_deposit_status(&mut self) {
        if self.add_step != AddStep::PriceDeposit {
            return;
        }
        if let Some(idx) = self.range_preset_applied {
            let (label, pct) = RANGE_PRESETS[idx];
            if pct.is_none() {
                self.status = "Full range — set starting price if new pool, then deposits".into();
            } else if self.needs_v3_starting_price() {
                self.status =
                    format!("Range {label} — edit starting price below, then deposit amounts");
            } else {
                self.status = format!("Range {label} — enter deposit amounts (Tab between fields)");
            }
        } else {
            match self.pool_lifecycle {
                Some(V3PoolLifecycle::Ready) => {
                    self.status = format!(
                        "{} · price loaded · pick a range preset (a to fine-tune later)",
                        self.venue.label()
                    );
                }
                Some(V3PoolLifecycle::Missing) => {
                    self.status =
                        "New pool — pick range preset · set starting price · then deposit".into();
                }
                Some(V3PoolLifecycle::Uninitialized { .. }) => {
                    self.status =
                        "Pool needs starting price — pick range preset · then deposit".into();
                }
                None => {}
            }
        }
        if self.pool_quote_inflight {
            self.status.push_str(" · verifying on chain…");
        }
    }

    fn resync_range_bounds_from_preset(&mut self) {
        let Some(idx) = self.range_preset_applied else {
            return;
        };
        let (_, pct) = RANGE_PRESETS[idx];
        match pct {
            None => {
                self.min_price.set_value("");
                self.max_price.set_value("");
            }
            Some(p) => {
                let center = self.center_price_f64().unwrap_or(1.0);
                let (min, max) = display_price_range_from_preset(center, p);
                self.min_price.set_value(trim_float_string(min));
                self.max_price.set_value(trim_float_string(max));
            }
        }
    }

    fn on_v3_price_field_edited(&mut self) {
        match self.focus {
            Focus::InitialPrice => self.resync_range_bounds_from_preset(),
            Focus::MinPrice | Focus::MaxPrice => self.clear_range_preset_if_price_edited(),
            _ => {}
        }
    }

    fn clear_range_preset_if_price_edited(&mut self) {
        self.range_preset_applied = None;
    }

    fn field_label_span(label: &str) -> Span<'static> {
        Span::styled(
            format!("{label}: "),
            Style::default().add_modifier(Modifier::BOLD),
        )
    }

    fn field_label_short_span(label: &str) -> Span<'static> {
        Span::styled(
            format!("{label}: "),
            Style::default().add_modifier(Modifier::BOLD),
        )
    }

    fn token_symbol<'a>(&self, input: &Input, assets: &'a [Balance]) -> &'a str {
        let raw = input.value().trim();
        token_symbol_for_address(assets, raw)
            .or_else(|| crate::views::token_symbol_hint(raw, self.chain_id))
            .unwrap_or("???")
    }

    /// Token picker box (First / Second token).
    #[allow(clippy::too_many_arguments)]
    fn render_token_field(
        &self,
        frame: &mut Frame,
        area: Rect,
        label: &str,
        input: &Input,
        focused: bool,
        assets: &[Balance],
        editing: bool,
        screen_width: u16,
    ) {
        let inner = brand::render_field_box(frame, area, focused);

        if focused && editing {
            let mut spans = vec![Self::field_label_span(label)];
            spans.extend(input.line().spans);
            frame.render_widget(Paragraph::new(Line::from(spans)), inner);
            return;
        }

        let raw = input.value().trim();
        if raw.is_empty() {
            let mut spans = vec![Self::field_label_span(label)];
            if focused {
                spans.extend(input.line().spans);
            } else {
                spans.push(Span::styled("Select", Style::default().fg(Color::DarkGray)));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), inner);
            return;
        }

        let sym = self.token_symbol(input, assets);
        frame.render_widget(
            Paragraph::new(brand::colored_token_address_under_augha(
                sym,
                raw,
                screen_width,
                inner.x,
            )),
            inner,
        );
        let label_w = (label.chars().count() + 1) as u16;
        frame.render_widget(
            Paragraph::new(Line::from(Self::field_label_short_span(label))),
            Rect {
                x: inner.x,
                y: inner.y,
                width: label_w.min(inner.width),
                height: 1,
            },
        );
    }

    fn tab_bar(&self) -> String {
        match self.stack {
            LpStack::V3 { .. } => Tab::v3_cycle()
                .iter()
                .map(|t| {
                    if *t == self.tab {
                        format!("[{}]", t.label())
                    } else {
                        t.label().to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" · "),
            LpStack::V2 { .. } => Tab::v2_cycle()
                .iter()
                .map(|t| {
                    if *t == self.tab {
                        format!("[{}]", t.label())
                    } else {
                        t.label().to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" · "),
        }
    }

    fn body_lines(&self) -> Vec<Line<'static>> {
        let target = match self.stack {
            LpStack::V3 { .. } => venue_position_manager(self.venue, self.chain_id)
                .map(|a| format!("NPM {a:#x}"))
                .unwrap_or_else(|| "—".into()),
            LpStack::V2 { .. } => venue_swap_router(self.venue, DexProtocol::V2, self.chain_id)
                .map(|a| format!("router {a:#x}"))
                .unwrap_or_else(|| "—".into()),
        };
        let mut out = vec![
            Line::from(vec![
                Span::styled("LP ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!(
                    "{} · {} · {} · {}",
                    self.venue.label(),
                    self.stack.label(),
                    chain_label(self.chain_id),
                    target
                )),
            ]),
            Line::from(vec![
                Span::styled("Tab ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!("{}   (←→)", self.tab_bar())),
            ]),
        ];
        match self.tab {
            Tab::List => self.render_list(&mut out),
            Tab::Increase => self.render_v3_increase(&mut out),
            Tab::Decrease => self.render_v3_decrease(&mut out),
            Tab::Collect => self.render_v3_collect(&mut out),
            Tab::Remove => self.render_v2_remove(&mut out),
            Tab::AddLp => {}
        }
        out
    }

    fn render_list(&self, out: &mut Vec<Line<'static>>) {
        out.push(Line::from(
            "↑↓ select position · Enter actions on other tabs",
        ));
        if !self.lp_supported() {
            out.push(Line::from("(LP not available on this network)"));
            return;
        }
        match self.stack {
            LpStack::V3 { .. } => {
                if self.v3_positions.is_empty() {
                    out.push(Line::from("(no positions — Add LP tab or r reload)"));
                } else {
                    for (i, p) in self.v3_positions.iter().enumerate() {
                        let mark = if i == self.sel { "▸" } else { " " };
                        out.push(Line::from(format!(
                            "{mark} #{} fee={} liq={} owed0={} owed1={}",
                            p.token_id, p.fee, p.liquidity, p.tokens_owed0, p.tokens_owed1
                        )));
                    }
                }
            }
            LpStack::V2 { .. } => {
                if self.v2_positions.is_empty() {
                    out.push(Line::from("(no positions — Add LP tab or r reload)"));
                } else {
                    for (i, p) in self.v2_positions.iter().enumerate() {
                        let mark = if i == self.sel { "▸" } else { " " };
                        out.push(Line::from(format!(
                            "{mark} pair={:#x} t0={:#x} t1={:#x} lp={}",
                            p.pair, p.token0, p.token1, p.lp_balance
                        )));
                    }
                }
            }
        }
    }

    fn render_v3_increase(&self, out: &mut Vec<Line<'static>>) {
        if let Some(p) = self.v3_positions.get(self.sel) {
            out.push(Line::from(format!(
                "Increase liquidity for NFT #{} (fee {})",
                p.token_id, p.fee
            )));
        } else {
            out.push(Line::from("Select a position on List tab first"));
        }
        out.push(Line::from("Approve extra token0/token1 to NPM if needed"));
        out.push(Line::from(format!("amount0: {}", self.amount0.value())));
        out.push(Line::from(format!("amount1: {}", self.amount1.value())));
    }

    fn render_v3_decrease(&self, out: &mut Vec<Line<'static>>) {
        if let Some(p) = self.v3_positions.get(self.sel) {
            out.push(Line::from(format!(
                "Decrease liquidity for NFT #{} (liq={})",
                p.token_id, p.liquidity
            )));
        } else {
            out.push(Line::from("Select a position on List tab first"));
        }
        out.push(Line::from(format!(
            "liquidity units: {}",
            self.liquidity.value()
        )));
        out.push(Line::from("Follow with Collect tab for owed tokens"));
    }

    fn render_v3_collect(&self, out: &mut Vec<Line<'static>>) {
        if let Some(p) = self.v3_positions.get(self.sel) {
            out.push(Line::from(format!(
                "Collect fees for NFT #{} (fee {})",
                p.token_id, p.fee
            )));
        } else {
            out.push(Line::from("Select a position on List tab first"));
        }
        out.push(Line::from("Enter · collect"));
    }

    fn render_v2_remove(&self, out: &mut Vec<Line<'static>>) {
        if let Some(p) = self.v2_positions.get(self.sel) {
            out.push(Line::from(format!(
                "Remove liquidity from pair {:#x}",
                p.pair
            )));
        } else {
            out.push(Line::from("Select a position on List tab first"));
        }
        out.push(Line::from(format!(
            "LP amount (raw): {}",
            self.liquidity.value()
        )));
        out.push(Line::from("Approve pair LP token to router if needed"));
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &WalletState,
        handle: &Handle,
        events: &EventBus,
    ) -> KeyOutcome {
        let _ = (handle, events);
        let tab_focus = matches!(key.code, KeyCode::Tab | KeyCode::BackTab);
        if self.busy == Busy::Sending {
            return KeyOutcome::Consumed;
        }
        if self.stage == Stage::Confirm {
            return self.handle_confirm(key, wallet);
        }
        if self.busy == Busy::Loading && !(self.on_v3_price_deposit() || tab_focus) {
            return KeyOutcome::Consumed;
        }

        if self.tab == Tab::AddLp {
            return self.handle_add_lp_key(key, wallet);
        }

        match key.code {
            KeyCode::Up if self.tab == Tab::List => {
                let len = match self.stack {
                    LpStack::V3 { .. } => self.v3_positions.len(),
                    LpStack::V2 { .. } => self.v2_positions.len(),
                };
                if len > 0 && self.sel > 0 {
                    self.sel -= 1;
                    self.sync_liquidity_from_selection();
                }
                KeyOutcome::Consumed
            }
            KeyCode::Down if self.tab == Tab::List => {
                let len = match self.stack {
                    LpStack::V3 { .. } => self.v3_positions.len(),
                    LpStack::V2 { .. } => self.v2_positions.len(),
                };
                if self.sel + 1 < len {
                    self.sel += 1;
                    self.sync_liquidity_from_selection();
                }
                KeyOutcome::Consumed
            }
            KeyCode::Left => {
                self.tab = self.tab.prev(self.stack);
                self.on_tab_changed();
                KeyOutcome::Consumed
            }
            KeyCode::Right => {
                self.tab = self.tab.next(self.stack);
                self.on_tab_changed();
                KeyOutcome::Consumed
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if let Some(job) = self.list_job(wallet) {
                    self.busy = Busy::Loading;
                    self.status = "Loading positions…".into();
                    KeyOutcome::StartJob(job)
                } else {
                    self.status = self.default_status_hint();
                    KeyOutcome::Consumed
                }
            }
            KeyCode::Enter => self.submit_manage(wallet),
            KeyCode::Esc => KeyOutcome::Back,
            _ => KeyOutcome::NotHandled,
        }
    }

    fn handle_add_lp_key(&mut self, key: KeyEvent, wallet: &WalletState) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                if self.focus != Focus::None {
                    self.deselect_focus();
                    KeyOutcome::Consumed
                } else if matches!(self.stack, LpStack::V3 { .. })
                    && self.add_step == AddStep::PriceDeposit
                {
                    self.add_step = AddStep::SelectPair;
                    self.clear_pool_quote();
                    KeyOutcome::Consumed
                } else {
                    KeyOutcome::Back
                }
            }
            KeyCode::Up | KeyCode::Down => {
                if self.cycle_venue_selector(matches!(key.code, KeyCode::Down)) {
                    KeyOutcome::Consumed
                } else {
                    KeyOutcome::NotHandled
                }
            }
            KeyCode::Left
                if self.focus == Focus::None && self.add_step != AddStep::PriceDeposit =>
            {
                self.tab = self.tab.prev(self.stack);
                self.on_tab_changed();
                KeyOutcome::Consumed
            }
            KeyCode::Right
                if self.focus == Focus::None && self.add_step != AddStep::PriceDeposit =>
            {
                self.tab = self.tab.next(self.stack);
                self.on_tab_changed();
                KeyOutcome::Consumed
            }
            KeyCode::Left | KeyCode::Right
                if self.add_step == AddStep::PriceDeposit
                    && matches!(self.focus, Focus::None | Focus::RangePresets) =>
            {
                if self.focus == Focus::None {
                    self.focus = Focus::RangePresets;
                }
                self.cycle_range_preset_highlight(matches!(key.code, KeyCode::Right));
                self.apply_range_preset(self.range_preset_idx);
                KeyOutcome::Consumed
            }
            KeyCode::Left | KeyCode::Right if self.focus == Focus::Fee => {
                self.cycle_fee(matches!(key.code, KeyCode::Right));
                KeyOutcome::Consumed
            }
            KeyCode::Left | KeyCode::Right if self.focus == Focus::Venue => KeyOutcome::Consumed,
            KeyCode::Left | KeyCode::Right => {
                let input = self.focused_input_mut();
                if let Some(input) = input {
                    match input.handle_key(key) {
                        InputAction::Ignored => KeyOutcome::NotHandled,
                        InputAction::Consumed | InputAction::Submitted => {
                            self.on_manual_token_edit();
                            if matches!(
                                self.focus,
                                Focus::InitialPrice | Focus::MinPrice | Focus::MaxPrice
                            ) {
                                self.on_v3_price_field_edited();
                            }
                            KeyOutcome::Consumed
                        }
                    }
                } else {
                    KeyOutcome::Consumed
                }
            }
            KeyCode::Tab => {
                self.normalize_v3_price_focus();
                let old = self.focus;
                self.focus = self.focus_tab_forward();
                self.on_focus_left(old);
                KeyOutcome::Consumed
            }
            KeyCode::BackTab => {
                self.normalize_v3_price_focus();
                let old = self.focus;
                self.focus = self.focus_tab_backward();
                self.on_focus_left(old);
                KeyOutcome::Consumed
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if let Some(job) = self.list_job(wallet) {
                    self.busy = Busy::Loading;
                    self.status = "Loading positions…".into();
                    KeyOutcome::StartJob(job)
                } else {
                    KeyOutcome::Consumed
                }
            }
            KeyCode::Enter if self.focus == Focus::RangePresets => {
                self.apply_range_preset(self.range_preset_idx);
                let old = self.focus;
                self.on_focus_left(old);
                self.focus = self.focus_tab_forward();
                KeyOutcome::Consumed
            }
            KeyCode::Enter
                if self.on_v3_price_deposit()
                    && matches!(
                        self.focus,
                        Focus::InitialPrice
                            | Focus::MinPrice
                            | Focus::MaxPrice
                            | Focus::Amount0
                            | Focus::Amount1
                    ) =>
            {
                let old = self.focus;
                self.on_focus_left(old);
                self.focus = self.focus_tab_forward();
                KeyOutcome::Consumed
            }
            KeyCode::Enter if self.focus != Focus::None => {
                self.deselect_focus();
                KeyOutcome::Consumed
            }
            KeyCode::Char(c)
                if self.add_step == AddStep::PriceDeposit
                    && matches!(self.focus, Focus::None | Focus::RangePresets)
                    && matches!(c, 'a' | 'A')
                    && matches!(self.stack, LpStack::V3 { .. }) =>
            {
                self.toggle_v3_custom_range();
                KeyOutcome::Consumed
            }
            KeyCode::Enter => self.submit_add_lp(wallet),
            _ => {
                if matches!(
                    self.focus,
                    Focus::None | Focus::Fee | Focus::Venue | Focus::RangePresets
                ) {
                    return KeyOutcome::NotHandled;
                }
                let (input, pick) = match self.focus {
                    Focus::Token0 => (Some(&mut self.token0), Some(&mut self.token0_pick)),
                    Focus::Token1 => (Some(&mut self.token1), Some(&mut self.token1_pick)),
                    Focus::InitialPrice => (Some(&mut self.initial_price), None),
                    Focus::MinPrice => (Some(&mut self.min_price), None),
                    Focus::MaxPrice => (Some(&mut self.max_price), None),
                    Focus::Ratio => (Some(&mut self.ratio), None),
                    Focus::Amount0 => (Some(&mut self.amount0), None),
                    Focus::Amount1 => (Some(&mut self.amount1), None),
                    Focus::None | Focus::Fee | Focus::Venue | Focus::RangePresets => unreachable!(),
                };
                let Some(input) = input else {
                    return KeyOutcome::Consumed;
                };
                match input.handle_key(key) {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    InputAction::Submitted => {
                        self.deselect_focus();
                        KeyOutcome::Consumed
                    }
                    InputAction::Consumed => {
                        if let Some(p) = pick {
                            if manual_edit_resets_token_pick(key.code) {
                                *p = TOKEN_PICK_UNINIT;
                                match self.focus {
                                    Focus::Token0 => self.token0_editing = true,
                                    Focus::Token1 => self.token1_editing = true,
                                    _ => {}
                                }
                            }
                        }
                        if matches!(self.focus, Focus::Ratio | Focus::Amount0 | Focus::Amount1)
                            && manual_edit_resets_token_pick(key.code)
                        {
                            let on_v3_price = matches!(self.stack, LpStack::V3 { .. })
                                && self.add_step == AddStep::PriceDeposit;
                            match self.focus {
                                Focus::Amount0 if on_v3_price => self.sync_amount1_from_price(),
                                Focus::Amount1 if on_v3_price => self.sync_amount0_from_price(),
                                Focus::Ratio | Focus::Amount0 => self.sync_amount1_from_ratio(),
                                Focus::Amount1 if !on_v3_price => self.sync_ratio_from_amounts(),
                                _ => {}
                            }
                        }
                        if matches!(
                            self.focus,
                            Focus::InitialPrice | Focus::MinPrice | Focus::MaxPrice
                        ) {
                            self.on_v3_price_field_edited();
                        }
                        KeyOutcome::Consumed
                    }
                }
            }
        }
    }

    fn on_manual_token_edit(&mut self) {
        match self.focus {
            Focus::Token0 => {
                self.token0_pick = TOKEN_PICK_UNINIT;
                self.token0_editing = true;
            }
            Focus::Token1 => {
                self.token1_pick = TOKEN_PICK_UNINIT;
                self.token1_editing = true;
            }
            _ => {}
        }
    }

    fn focused_input_mut(&mut self) -> Option<&mut Input> {
        match self.focus {
            Focus::Token0 => Some(&mut self.token0),
            Focus::Token1 => Some(&mut self.token1),
            Focus::InitialPrice => Some(&mut self.initial_price),
            Focus::MinPrice => Some(&mut self.min_price),
            Focus::MaxPrice => Some(&mut self.max_price),
            Focus::Ratio => Some(&mut self.ratio),
            Focus::Amount0 => Some(&mut self.amount0),
            Focus::Amount1 => Some(&mut self.amount1),
            Focus::None | Focus::Fee | Focus::Venue | Focus::RangePresets => None,
        }
    }

    /// ↑/↓ on the venue row during Add LP pair selection.
    pub fn cycle_venue_selector(&mut self, forward: bool) -> bool {
        if self.tab != Tab::AddLp
            || self.stage != Stage::Input
            || !matches!(self.stack, LpStack::V3 { .. })
            || self.add_step != AddStep::SelectPair
        {
            return false;
        }
        if !matches!(self.focus, Focus::None | Focus::Venue) {
            return false;
        }
        self.cycle_v3_venue(forward);
        self.focus = Focus::Venue;
        true
    }

    fn cycle_v3_venue(&mut self, forward: bool) {
        let venues = lp_v3_venue_picker(self.chain_id);
        if venues.is_empty() {
            self.status = format!("No V3 LP venues on {}", chain_label(self.chain_id));
            return;
        }
        if venues.len() == 1 {
            self.status = format!(
                "Only {} on {} — ↑↓ has no other venue",
                venues[0].label(),
                chain_label(self.chain_id)
            );
            return;
        }
        let idx = venues.iter().position(|v| *v == self.venue).unwrap_or(0);
        let next = if forward {
            (idx + 1) % venues.len()
        } else {
            (idx + venues.len() - 1) % venues.len()
        };
        let picked = venues[next];
        self.venue = picked;
        self.stack = LpStack::V3 { venue: self.venue };
        if venue_position_manager(picked, self.chain_id).is_none() {
            self.status = format!(
                "{} V3 LP is on testnet 943 — F1 Network → PulseChain testnet",
                picked.label()
            );
            return;
        }
        self.apply_venue_token_defaults(picked == DexVenue::NineMm);
        self.status = format!("{} · ↑↓ venue · ←→ fee tier", self.venue.label());
    }

    fn cycle_fee(&mut self, forward: bool) {
        let Some(idx) = LP_FEE_TIERS.iter().position(|f| *f == self.fee_tier) else {
            self.fee_tier = 2500;
            return;
        };
        let next = if forward {
            (idx + 1) % LP_FEE_TIERS.len()
        } else {
            (idx + LP_FEE_TIERS.len() - 1) % LP_FEE_TIERS.len()
        };
        self.fee_tier = LP_FEE_TIERS[next];
        self.status = format!("Fee tier {} · ←→ cycle", fee_tier_display(self.fee_tier));
    }

    fn focus_tab_forward(&self) -> Focus {
        let on_price =
            matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::PriceDeposit;
        if on_price {
            if self.v3_custom_range {
                return match self.focus {
                    Focus::None => Focus::RangePresets,
                    Focus::RangePresets => Focus::MinPrice,
                    Focus::MinPrice => Focus::InitialPrice,
                    Focus::InitialPrice => Focus::MaxPrice,
                    Focus::MaxPrice => Focus::Amount0,
                    Focus::Amount0 => Focus::Amount1,
                    Focus::Amount1 => Focus::None,
                    _ => Focus::RangePresets,
                };
            }
            return match self.focus {
                Focus::None => {
                    if self.range_preset_applied.is_some() {
                        self.next_v3_focus_after_presets()
                    } else {
                        Focus::RangePresets
                    }
                }
                Focus::RangePresets => self.next_v3_focus_after_presets(),
                Focus::InitialPrice => Focus::Amount0,
                Focus::Amount0 => Focus::Amount1,
                Focus::Amount1 => Focus::None,
                _ => Focus::RangePresets,
            };
        }
        if matches!(self.stack, LpStack::V2 { .. }) {
            return match self.focus {
                Focus::None => Focus::Token0,
                Focus::Token0 => Focus::Token1,
                Focus::Token1 => Focus::Ratio,
                Focus::Ratio => Focus::Amount0,
                Focus::Amount0 => Focus::Amount1,
                Focus::Amount1 => Focus::None,
                _ => Focus::Token0,
            };
        }
        match self.focus {
            Focus::None => Focus::Venue,
            Focus::Venue => Focus::Token0,
            Focus::Token0 => Focus::Token1,
            Focus::Token1 => Focus::Fee,
            Focus::Fee => Focus::None,
            _ => Focus::Venue,
        }
    }

    fn focus_tab_backward(&self) -> Focus {
        let on_price =
            matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::PriceDeposit;
        if on_price {
            if self.v3_custom_range {
                return match self.focus {
                    Focus::None => Focus::Amount1,
                    Focus::Amount1 => Focus::Amount0,
                    Focus::Amount0 => Focus::MaxPrice,
                    Focus::MaxPrice => Focus::InitialPrice,
                    Focus::InitialPrice => Focus::MinPrice,
                    Focus::MinPrice => Focus::RangePresets,
                    Focus::RangePresets => Focus::None,
                    _ => Focus::Amount0,
                };
            }
            return match self.focus {
                Focus::None => Focus::Amount1,
                Focus::Amount1 => Focus::Amount0,
                Focus::Amount0 => {
                    if self.needs_v3_starting_price() {
                        Focus::InitialPrice
                    } else {
                        Focus::RangePresets
                    }
                }
                Focus::InitialPrice => Focus::RangePresets,
                Focus::RangePresets => Focus::None,
                _ => Focus::Amount0,
            };
        }
        if matches!(self.stack, LpStack::V2 { .. }) {
            return match self.focus {
                Focus::None => Focus::Amount1,
                Focus::Amount1 => Focus::Amount0,
                Focus::Amount0 => Focus::Ratio,
                Focus::Ratio => Focus::Token1,
                Focus::Token1 => Focus::Token0,
                Focus::Token0 => Focus::None,
                _ => Focus::Amount1,
            };
        }
        match self.focus {
            Focus::None => Focus::Fee,
            Focus::Fee => Focus::Token1,
            Focus::Token1 => Focus::Token0,
            Focus::Token0 => Focus::Venue,
            Focus::Venue => Focus::None,
            _ => Focus::Fee,
        }
    }

    fn on_focus_left(&mut self, old: Focus) {
        let on_v3_price =
            matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::PriceDeposit;
        match old {
            Focus::Token0 => self.token0_editing = false,
            Focus::Token1 => self.token1_editing = false,
            Focus::Ratio | Focus::Amount0 if on_v3_price => self.sync_amount1_from_price(),
            Focus::Amount1 if on_v3_price => self.sync_amount0_from_price(),
            Focus::Ratio | Focus::Amount0 => self.sync_amount1_from_ratio(),
            Focus::Amount1 if !on_v3_price => self.sync_ratio_from_amounts(),
            Focus::InitialPrice | Focus::MinPrice | Focus::MaxPrice if on_v3_price => {
                self.sync_amount1_from_price();
            }
            _ => {}
        }
    }

    fn deselect_focus(&mut self) {
        let old = self.focus;
        self.on_focus_left(old);
        self.focus = Focus::None;
    }

    fn validate_pair_selection(&self) -> Result<(), String> {
        let t0 = parse_token_address(self.token0.value(), "first token")?;
        let t1 = parse_token_address(self.token1.value(), "second token")?;
        if t0 == t1 {
            return Err("Pick two different tokens".into());
        }
        Ok(())
    }

    fn submit_add_lp(&mut self, wallet: &WalletState) -> KeyOutcome {
        if !self.lp_supported() {
            self.status = self.default_status_hint();
            return KeyOutcome::Consumed;
        }
        if matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::SelectPair {
            match self.validate_pair_selection() {
                Ok(()) => {
                    self.add_step = AddStep::PriceDeposit;
                    self.focus = Focus::None;
                    self.begin_optimistic_pool_preview();
                    if self.range_preset_applied.is_none() {
                        self.apply_range_preset(5);
                    }
                    self.sync_amount1_from_price();
                    self.refresh_price_deposit_status();
                    self.focus_v3_deposit_after_preset();
                    return self.spawn_pool_quote_job(wallet);
                }
                Err(e) => self.status = e,
            }
            return KeyOutcome::Consumed;
        }
        match self.stack {
            LpStack::V3 { .. } => self.start_add_liquidity_job(wallet),
            LpStack::V2 { .. } => match self.build_v2_add_tx(wallet) {
                Ok(tx) => self.confirm_tx(tx, "add liquidity"),
                Err(e) => {
                    self.status = e;
                    KeyOutcome::Consumed
                }
            },
        }
    }

    fn submit_manage(&mut self, wallet: &WalletState) -> KeyOutcome {
        if !self.lp_supported() {
            self.status = self.default_status_hint();
            return KeyOutcome::Consumed;
        }
        match self.tab {
            Tab::List => KeyOutcome::Consumed,
            Tab::Increase => match self.build_increase_tx(wallet) {
                Ok(tx) => self.confirm_tx(tx, "increase"),
                Err(e) => {
                    self.status = e;
                    KeyOutcome::Consumed
                }
            },
            Tab::Decrease => match self.build_decrease_tx(wallet) {
                Ok(tx) => self.confirm_tx(tx, "decrease"),
                Err(e) => {
                    self.status = e;
                    KeyOutcome::Consumed
                }
            },
            Tab::Collect => match self.build_collect_tx(wallet) {
                Ok(tx) => self.confirm_tx(tx, "collect"),
                Err(e) => {
                    self.status = e;
                    KeyOutcome::Consumed
                }
            },
            Tab::Remove => match self.build_v2_remove_tx(wallet) {
                Ok(tx) => self.confirm_tx(tx, "remove liquidity"),
                Err(e) => {
                    self.status = e;
                    KeyOutcome::Consumed
                }
            },
            Tab::AddLp => KeyOutcome::Consumed,
        }
    }

    fn start_add_liquidity_job(&mut self, wallet: &WalletState) -> KeyOutcome {
        if self.pool_quote_inflight {
            self.status = "Still verifying pool on chain… wait a moment, then Enter again".into();
            return KeyOutcome::Consumed;
        }
        if let Err(e) = self.validate_pair_selection() {
            self.status = e;
            return KeyOutcome::Consumed;
        }
        if self.amount0.value().trim().is_empty() || self.amount1.value().trim().is_empty() {
            self.status = "Enter both deposit amounts".into();
            return KeyOutcome::Consumed;
        }
        self.lp_deploy_active = true;
        self.lp_deploy_pending_resume = false;
        self.lp_deploy_last_step = LpDeployLastStep::None;
        self.busy = Busy::Loading;
        self.status = "Checking pool…".into();
        match self.build_lp_deploy_job(wallet, V3LpDeployWait::None) {
            Ok(job) => KeyOutcome::StartJob(job),
            Err(e) => {
                self.busy = Busy::Idle;
                self.lp_deploy_active = false;
                self.status = e;
                KeyOutcome::Consumed
            }
        }
    }

    fn sorted_pair(&self) -> Result<SortedPair, String> {
        let first = parse_token_address(self.token0.value(), "first token")?;
        let second = parse_token_address(self.token1.value(), "second token")?;
        let dec_first = Self::parse_decimals(self.dec0.value(), "decimals0")?;
        let dec_second = Self::parse_decimals(self.dec1.value(), "decimals1")?;
        if first < second {
            Ok(SortedPair {
                token0: first,
                token1: second,
                dec0: dec_first,
                dec1: dec_second,
                first_is_token0: true,
            })
        } else {
            Ok(SortedPair {
                token0: second,
                token1: first,
                dec0: dec_second,
                dec1: dec_first,
                first_is_token0: false,
            })
        }
    }

    fn user_price_to_pool_price(
        &self,
        first_is_token0: bool,
        user_price: &str,
    ) -> Result<String, String> {
        let raw = user_price.trim();
        if raw.is_empty() {
            return Ok(String::new());
        }
        let p: f64 = raw
            .parse()
            .map_err(|_| "Invalid price — use decimal e.g. 0.00126532".to_string())?;
        if p <= 0.0 {
            return Err("Price must be > 0".into());
        }
        let pool = if first_is_token0 { p } else { 1.0 / p };
        Ok(trim_float_string(pool))
    }

    fn pool_price_to_user_price(&self, first_is_token0: bool, pool_price: &str) -> String {
        let Ok(p) = pool_price.trim().parse::<f64>() else {
            return pool_price.trim().to_string();
        };
        if p <= 0.0 {
            return pool_price.trim().to_string();
        }
        let user = if first_is_token0 { p } else { 1.0 / p };
        trim_float_string(user)
    }

    fn sync_amount1_from_ratio(&mut self) {
        let Ok(a0) = self.amount0.value().trim().parse::<f64>() else {
            return;
        };
        let Ok(r) = self.ratio.value().trim().parse::<f64>() else {
            return;
        };
        if a0 <= 0.0 || r <= 0.0 {
            return;
        }
        self.amount1.set_value(trim_float_string(a0 * r));
    }

    fn sync_ratio_from_amounts(&mut self) {
        let Ok(a0) = self.amount0.value().trim().parse::<f64>() else {
            return;
        };
        let Ok(a1) = self.amount1.value().trim().parse::<f64>() else {
            return;
        };
        if a0 <= 0.0 {
            return;
        }
        self.ratio.set_value(trim_float_string(a1 / a0));
    }

    fn confirm_tx(&mut self, tx: vaughan_core::chains::EvmTransaction, action: &str) -> KeyOutcome {
        self.pending_tx = Some(tx);
        self.confirm_lines = vec![Line::from(format!(
            "Confirm {} {} (Enter send · Esc cancel)",
            self.venue.label(),
            action
        ))];
        self.stage = Stage::Confirm;
        KeyOutcome::Consumed
    }

    fn parse_decimals(raw: &str, label: &str) -> Result<u8, String> {
        raw.trim()
            .parse::<u8>()
            .map_err(|_| format!("Invalid {label}"))
    }

    fn parse_liquidity_u128(raw: &str) -> Result<u128, String> {
        raw.trim()
            .parse()
            .map_err(|_| "Invalid liquidity".to_string())
    }

    fn parse_liquidity_u256(raw: &str) -> Result<U256, String> {
        U256::from_str(raw.trim()).map_err(|_| "Invalid LP amount".to_string())
    }

    fn build_increase_tx(
        &self,
        wallet: &WalletState,
    ) -> Result<vaughan_core::chains::EvmTransaction, String> {
        let pos = self
            .v3_positions
            .get(self.sel)
            .ok_or_else(|| "No position selected".to_string())?;
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let rpc = wallet.active_rpc_url();
        let amount0 = parse_swap_amount(self.amount0.value(), "amount0", 18)?;
        let amount1 = parse_swap_amount(self.amount1.value(), "amount1", 18)?;
        build_v3_increase_evm(
            &from,
            self.venue,
            self.chain_id,
            &rpc,
            pos.token_id,
            amount0,
            amount1,
            min_out_after_slippage(amount0, DEFAULT_DEX_SLIPPAGE_BPS),
            min_out_after_slippage(amount1, DEFAULT_DEX_SLIPPAGE_BPS),
            None,
        )
        .map_err(|e| e.user_message())
    }

    fn build_decrease_tx(
        &self,
        wallet: &WalletState,
    ) -> Result<vaughan_core::chains::EvmTransaction, String> {
        let pos = self
            .v3_positions
            .get(self.sel)
            .ok_or_else(|| "No position selected".to_string())?;
        let liquidity = Self::parse_liquidity_u128(self.liquidity.value())?;
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let rpc = wallet.active_rpc_url();
        build_v3_decrease_evm(
            &from,
            self.venue,
            self.chain_id,
            &rpc,
            pos.token_id,
            liquidity,
            U256::ZERO,
            U256::ZERO,
            None,
        )
        .map_err(|e| e.user_message())
    }

    fn build_collect_tx(
        &self,
        wallet: &WalletState,
    ) -> Result<vaughan_core::chains::EvmTransaction, String> {
        let pos = self
            .v3_positions
            .get(self.sel)
            .ok_or_else(|| "No position selected".to_string())?;
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let rpc = wallet.active_rpc_url();
        build_v3_collect_evm(
            &from,
            self.venue,
            self.chain_id,
            &rpc,
            pos.token_id,
            None,
            u128::MAX,
            u128::MAX,
        )
        .map_err(|e| e.user_message())
    }

    fn build_v2_add_tx(
        &self,
        wallet: &WalletState,
    ) -> Result<vaughan_core::chains::EvmTransaction, String> {
        self.validate_pair_selection()?;
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let token0 = parse_token_address(self.token0.value(), "first token")?;
        let token1 = parse_token_address(self.token1.value(), "second token")?;
        let dec0 = Self::parse_decimals(self.dec0.value(), "decimals0")?;
        let dec1 = Self::parse_decimals(self.dec1.value(), "decimals1")?;
        let wpls = wpls_for_chain(self.chain_id);
        let native = wpls.filter(|w| token0 == *w || token1 == *w);
        build_v2_add_liquidity_evm(
            &from,
            self.venue,
            self.chain_id,
            token0,
            token1,
            self.amount0.value().trim(),
            self.amount1.value().trim(),
            dec0,
            dec1,
            DEFAULT_DEX_SLIPPAGE_BPS,
            native,
        )
        .map_err(|e| e.user_message())
    }

    fn build_v2_remove_tx(
        &self,
        wallet: &WalletState,
    ) -> Result<vaughan_core::chains::EvmTransaction, String> {
        let pos = self
            .v2_positions
            .get(self.sel)
            .ok_or_else(|| "No position selected".to_string())?;
        let liquidity = Self::parse_liquidity_u256(self.liquidity.value())?;
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let wpls = wpls_for_chain(self.chain_id);
        let native = wpls.filter(|w| pos.token0 == *w || pos.token1 == *w);
        build_v2_remove_liquidity_evm(
            &from,
            self.venue,
            self.chain_id,
            pos.token0,
            pos.token1,
            liquidity,
            DEFAULT_DEX_SLIPPAGE_BPS,
            native,
        )
        .map_err(|e| e.user_message())
    }

    fn handle_confirm(&mut self, key: KeyEvent, _wallet: &WalletState) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                if self.lp_deploy_active {
                    self.lp_deploy_active = false;
                    self.lp_deploy_pending_resume = false;
                    self.lp_deploy_last_step = LpDeployLastStep::None;
                }
                self.stage = Stage::Input;
                self.pending_tx = None;
                self.confirm_lines.clear();
                KeyOutcome::Consumed
            }
            KeyCode::Enter => {
                let Some(tx) = self.pending_tx.take() else {
                    return KeyOutcome::Consumed;
                };
                self.busy = Busy::Sending;
                self.status = "Broadcasting…".into();
                KeyOutcome::StartJob(UiJob::SendEvm { tx })
            }
            _ => KeyOutcome::Consumed,
        }
    }
}

fn fee_tier_display(fee: u32) -> String {
    let pct = fee as f64 / 10_000.0;
    if pct >= 1.0 {
        format!("{pct:.1}%")
    } else {
        format!("{pct:.2}%")
    }
}

fn parse_price_f64(raw: &str) -> Result<f64, ()> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(());
    }
    s.parse::<f64>()
        .map_err(|_| ())
        .and_then(|p| if p > 0.0 { Ok(p) } else { Err(()) })
}

fn trim_float_string(v: f64) -> String {
    let s = format!("{v:.12}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
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
    fn v3_opens_on_add_lp_tab() {
        let v = LpView::for_chain(369);
        assert_eq!(v.tab, Tab::AddLp);
        assert_eq!(v.add_step, AddStep::SelectPair);
    }

    #[test]
    fn up_down_cycles_lp_venue_from_default_focus() {
        let mut v = LpView::for_chain(369);
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
    fn tab_from_none_skips_presets_when_already_applied() {
        let mut v = LpView::for_chain(369);
        v.add_step = AddStep::PriceDeposit;
        v.pool_lifecycle = Some(V3PoolLifecycle::Missing);
        v.range_preset_applied = Some(5);
        v.focus = Focus::None;
        assert_eq!(v.focus_tab_forward(), Focus::InitialPrice);
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
        assert_eq!(v.busy, Busy::Idle);
        assert!(v.status.contains("Ready — confirm createPool"));
        assert_eq!(v.stage, Stage::Confirm);
    }
}
