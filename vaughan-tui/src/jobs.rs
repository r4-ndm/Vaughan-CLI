//! Background UI jobs so RPC work never `block_on`s the TUI thread.

use vaughan_core::chains::{Balance, EvmTransaction, Fee};
use vaughan_core::core::StealthSendResult;
use vaughan_core::error::WalletError;
use vaughan_core::security::stealth::StealthAnnouncement;

/// Work the app should spawn on the tokio runtime.
pub enum UiJob {
    /// Native balance + gas hint for the always-on status chrome.
    RefreshChrome,
    RefreshBalance,
    RefreshAssets,
    EstimateFee {
        to: String,
        value_wei: String,
    },
    EstimateTokenFee {
        token: String,
        to: String,
        amount: String,
    },
    SendWithFee {
        to: String,
        value_wei: String,
        fee: Fee,
    },
    Send {
        to: String,
        value_wei: String,
    },
    SendToken {
        token: String,
        to: String,
        amount: String,
    },
    SendTokenWithFee {
        token: String,
        to: String,
        amount: String,
        fee: Fee,
    },
    SendStealth {
        announcement: StealthAnnouncement,
        value_wei: String,
    },
    /// Arbitrary EVM call (DEX swap, contract write) after explicit user confirm.
    SendEvm {
        tx: EvmTransaction,
    },
    /// Aggregator Pathfinder / PulseSwap quote (no signing).
    AggQuote {
        venue: vaughan_core::core::AggVenue,
        token_in: String,
        token_out: String,
        amount: String,
        slippage: f64,
        native_in: bool,
        native_out: bool,
        account: Option<String>,
    },
    /// LibertySwap cross-chain quote (no signing).
    BridgeQuote {
        src_token: String,
        dst_token: String,
        amount: String,
        src_chain: u64,
        dst_chain: u64,
        recipient: String,
    },
    /// Recent ERC-20 Transfer activity for History.
    RefreshActivity {
        limit: u32,
    },
    /// Scan known-spender allowances for Approvals.
    RefreshAllowances,
}

/// Cached need-to-know strip shared across every unlocked screen.
#[derive(Debug, Clone, Default)]
pub struct ChromeSnapshot {
    pub balance: Option<Balance>,
    /// Suggested max fee / gas price in gwei (display string, e.g. `"12.4"`).
    pub gas_gwei: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
    /// Which status box is hotkeyed (F1 / F2 / F3).
    pub focus: ChromeFocus,
    /// Assets with a balance (F2 ↑/↓). Filled by [`UiJob::RefreshAssets`].
    pub assets: Vec<Balance>,
    pub asset_idx: usize,
    pub assets_loading: bool,
    /// Pending F1 network list index (↑/↓ preview; Enter commits).
    pub pending_network_idx: Option<usize>,
    /// Pending F2 asset index.
    pub pending_asset_idx: Option<usize>,
    /// Pending F3 account index (`Account::index`).
    pub pending_account_index: Option<u32>,
    /// Count of MCP proposals waiting in the file queue.
    pub mcp_pending: usize,
}

/// Status-strip focus for F1 / F2 / F3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChromeFocus {
    #[default]
    None,
    /// F1 — cycle networks with ↑/↓, Enter to set.
    Network,
    /// F2 — cycle coins with balance with ↑/↓, Enter to set.
    Asset,
    /// F3 — cycle accounts with ↑/↓, Enter to set.
    Account,
}

/// Result delivered back to the UI thread via an unbounded channel.
pub enum UiJobResult {
    Chrome(Result<(Balance, String), WalletError>),
    Balance(Result<Balance, WalletError>),
    Assets(Result<Vec<Balance>, WalletError>),
    Fee(Result<Fee, WalletError>),
    Send(Result<String, WalletError>),
    SendStealth(Result<StealthSendResult, WalletError>),
    AggQuote(Result<vaughan_core::core::AggQuote, WalletError>),
    BridgeQuote(Box<Result<vaughan_core::core::BridgeQuote, WalletError>>),
    Activity(Result<Vec<vaughan_core::chains::TxRecord>, WalletError>),
    Allowances(Result<Vec<vaughan_core::chains::AllowanceEntry>, WalletError>),
}

/// Frames for a braille spinner (no extra deps).
pub fn spinner_frame(tick: u64) -> char {
    const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[(tick as usize) % FRAMES.len()]
}
