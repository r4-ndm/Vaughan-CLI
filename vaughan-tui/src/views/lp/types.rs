#![allow(unused_imports)]
use ratatui::text::Line;
use vaughan_core::core::{
    DexVenue, LpStack, V2LpPosition, V3LpDeployWait, V3LpPositionView, V3PoolLifecycle,
    V3PositionInfo,
};

use crate::input::Input;

/// HEX on PulseChain mainnet (8 decimals).
pub(crate) const HEX_MAINNET: &str = "0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";

/// 9inch V3 fee tiers on Pulse (0.01% … 2%).
pub(crate) const LP_FEE_TIERS: &[u32] = &[100, 500, 2500, 10_000, 20_000];

/// 9mm-style symmetric range shortcuts around the current price (`None` = full range).
pub(crate) const RANGE_PRESETS: &[(&str, Option<f64>)] = &[
    ("1%", Some(1.0)),
    ("2%", Some(2.0)),
    ("5%", Some(5.0)),
    ("10%", Some(10.0)),
    ("20%", Some(20.0)),
    ("50%", Some(50.0)),
    ("Full", None),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    List,
    AddLp,
    Increase,
    Decrease,
    Collect,
    Remove,
}

impl Tab {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::AddLp => "Add LP",
            Self::Increase => "Increase",
            Self::Decrease => "Decrease",
            Self::Collect => "Collect",
            Self::Remove => "Remove",
        }
    }

    pub(crate) fn v3_cycle() -> &'static [Self] {
        &[
            Self::List,
            Self::AddLp,
            Self::Increase,
            Self::Decrease,
            Self::Collect,
        ]
    }

    pub(crate) fn v2_cycle() -> &'static [Self] {
        &[Self::List, Self::AddLp, Self::Remove]
    }

    pub(crate) fn next(self, stack: LpStack) -> Self {
        let tabs = match stack {
            LpStack::V3 { .. } => Self::v3_cycle(),
            LpStack::V2 { .. } => Self::v2_cycle(),
        };
        let idx = tabs.iter().position(|t| *t == self).unwrap_or(0);
        tabs[(idx + 1) % tabs.len()]
    }

    pub(crate) fn prev(self, stack: LpStack) -> Self {
        let tabs = match stack {
            LpStack::V3 { .. } => Self::v3_cycle(),
            LpStack::V2 { .. } => Self::v2_cycle(),
        };
        let idx = tabs.iter().position(|t| *t == self).unwrap_or(0);
        tabs[(idx + tabs.len() - 1) % tabs.len()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Input,
    Confirm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Busy {
    Idle,
    Loading,
    EstimatingFee,
    Sending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LpConfirmFocus {
    Speed,
    CustomGas,
}

/// What the user is approving on the confirm screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LpConfirmAction {
    /// PancakeSwap-style **Enable** on the deposit form (before review).
    Enable {
        symbol: String,
        label: String,
    },
    /// PancakeSwap-style preview before the on-chain pipeline (one review for the deposit).
    AddReview,
    Deploy {
        step: LpDeployLastStep,
        label: String,
    },
    Increase,
    Decrease,
    Collect,
    V2Add,
    V2Remove,
}

/// V3 add-LP: review once (PCS preview modal), then execute setup/approve/mint steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LpPipelinePhase {
    #[default]
    None,
    Review,
    Execute,
}

/// V3 add flow: pair + fee, then price range + deposits (9inch / 9mm style).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddStep {
    SelectPair,
    PriceDeposit,
}

/// Partial remove shortcuts on Decrease / V2 Remove (percent of position liquidity).
pub(crate) const DECREASE_PRESETS: &[(&str, u8)] =
    &[("25%", 25), ("50%", 50), ("75%", 75), ("Max", 100)];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Focus {
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
    /// Custom remove amount on Decrease / V2 Remove tabs.
    Liquidity,
}

pub(crate) struct SortedPair {
    pub(crate) token0: alloy::primitives::Address,
    pub(crate) token1: alloy::primitives::Address,
    pub(crate) dec0: u8,
    pub(crate) dec1: u8,
    pub(crate) first_is_token0: bool,
}

pub(crate) struct V3DepositPreviewContext {
    pub(crate) pair: SortedPair,
    pub(crate) pool_min: String,
    pub(crate) pool_max: String,
    pub(crate) sqrt: alloy::primitives::U160,
    pub(crate) tick: i32,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) enum LpDeployLastStep {
    #[default]
    None,
    CreatePool,
    Initialize,
    Approve,
    AddLiquidity,
}

impl LpDeployLastStep {
    pub(crate) fn from_deploy_label(label: &str) -> Self {
        match label {
            "createPool" => Self::CreatePool,
            "initialize" => Self::Initialize,
            "add liquidity" => Self::AddLiquidity,
            _ if label.starts_with("approve") => Self::Approve,
            _ => Self::None,
        }
    }
}

/// Gas + summary state for the LP confirm screen.
pub(crate) struct LpConfirmUi {
    pub(crate) action: LpConfirmAction,
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) pending_tx: vaughan_core::chains::EvmTransaction,
    pub(crate) pending_fee_estimate: Option<vaughan_core::chains::EvmTransaction>,
    pub(crate) base_fee: Option<vaughan_core::chains::Fee>,
    pub(crate) speed: vaughan_core::chains::FeeSpeed,
    pub(crate) custom_gas: Input,
    pub(crate) focus: LpConfirmFocus,
    /// Pipeline step after review — reuse gas from review, no speed picker.
    pub(crate) pipeline_step: bool,
}

pub struct LpView {
    pub(crate) stack: LpStack,
    pub(crate) venue: DexVenue,
    pub(crate) tab: Tab,
    pub(crate) stage: Stage,
    pub(crate) busy: Busy,
    pub(crate) tick: u64,
    pub(crate) status: String,
    pub(crate) chain_id: u64,
    pub(crate) v3_positions: Vec<V3LpPositionView>,
    pub(crate) v2_positions: Vec<V2LpPosition>,
    /// Bumped on each list request; drop stale job results after F3 switch.
    pub(crate) list_gen: u64,
    pub(crate) sel: usize,
    /// When `Some`, List has focused a position and ↑↓ picks a manage action.
    pub(crate) list_action_idx: Option<usize>,
    pub(crate) add_step: AddStep,
    pub(crate) focus: Focus,
    pub(crate) token0: Input,
    pub(crate) token1: Input,
    pub(crate) token0_pick: usize,
    pub(crate) token1_pick: usize,
    pub(crate) token0_editing: bool,
    pub(crate) token1_editing: bool,
    pub(crate) fee_tier: u32,
    pub(crate) initial_price: Input,
    pub(crate) min_price: Input,
    pub(crate) max_price: Input,
    pub(crate) ratio: Input,
    pub(crate) amount0: Input,
    pub(crate) amount1: Input,
    pub(crate) dec0: Input,
    pub(crate) dec1: Input,
    pub(crate) liquidity: Input,
    /// Non-interactive status lines (e.g. between deploy steps).
    pub(crate) confirm_lines: Vec<Line<'static>>,
    pub(crate) confirm_ui: Option<Box<LpConfirmUi>>,
    /// Highlighted preset chip when [`Focus::RangePresets`].
    pub(crate) range_preset_idx: usize,
    /// Applied preset index, or `None` after manual min/max/current edits.
    pub(crate) range_preset_applied: Option<usize>,
    /// Highlighted partial-remove preset on Decrease / Remove tabs.
    pub(crate) decrease_preset_idx: usize,
    /// Applied decrease preset, or `None` after manual remove-unit edits.
    pub(crate) decrease_preset_applied: Option<usize>,
    /// On-chain pool lifecycle for V3 deposit coupling (after [`UiJob::LpV3PoolQuote`]).
    pub(crate) pool_lifecycle: Option<V3PoolLifecycle>,
    pub(crate) pool_sqrt_x96: Option<alloy::primitives::U160>,
    pub(crate) pool_tick: Option<i32>,
    /// Background [`UiJob::LpV3PoolQuote`] in flight (step 2 stays interactive).
    pub(crate) pool_quote_inflight: bool,
    /// Preset-first range; press `a` to reveal min/current/max for fine-tuning.
    pub(crate) v3_custom_range: bool,
    /// Multi-step V3 deploy (createPool → initialize → approve → mint).
    pub(crate) lp_deploy_active: bool,
    pub(crate) lp_deploy_pending_resume: bool,
    pub(crate) lp_deploy_last_step: LpDeployLastStep,
    /// Step label frozen when the user presses Enter on confirm (avoids race with follow-up jobs).
    pub(crate) lp_deploy_sent_step: LpDeployLastStep,
    /// PancakeSwap-style add flow: Review → Execute pipeline.
    pub(crate) lp_pipeline_phase: LpPipelinePhase,
    pub(crate) lp_pipeline_speed: vaughan_core::chains::FeeSpeed,
    pub(crate) lp_pipeline_custom_gwei: String,
    /// Label of the deploy step the user last confirmed (for on-chain wait).
    pub(crate) lp_deploy_last_label: String,
    /// Wait kind queued after a successful broadcast (from frozen sent step).
    pub(crate) lp_deploy_followup_wait: Option<V3LpDeployWait>,
    /// Form token0 (first field) enabled for NPM — `None` = not checked / N/A (new pool).
    pub(crate) lp_enable_first: Option<bool>,
    /// Form token1 (second field) enabled for NPM.
    pub(crate) lp_enable_second: Option<bool>,
    pub(crate) lp_enable_check_inflight: bool,
    pub(crate) lp_enable_pending_resume: bool,
    pub(crate) lp_enable_recheck_pending: bool,
    pub(crate) lp_enable_last_label: String,
    pub(crate) lp_enable_in_confirm: bool,
    /// Reload the position list after a successful manage tx (decrease/collect/…).
    pub(crate) lp_reload_pending: bool,
}
