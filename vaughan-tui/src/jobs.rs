//! Background UI jobs so RPC work never `block_on`s the TUI thread.

use secrecy::SecretString;
use vaughan_core::chains::{Balance, EvmTransaction, Fee};
use vaughan_core::core::{AccountManager, OperatingMode};
use vaughan_core::core::{BroadcastEntry, BroadcastReceipt, ReplaceKind, StealthSendResult};
use vaughan_core::error::WalletError;
use vaughan_core::security::stealth::StealthAnnouncement;

/// Work the app should spawn on the tokio runtime.
pub enum UiJob {
    /// Vault KDF + account derivation (Argon2id — seconds in a debug build).
    /// Carries the session mode to apply on success; the wallet itself is
    /// only locked briefly to clone the [`vaughan_core::core::UnlockPayload`].
    Unlock {
        password: SecretString,
        mode: OperatingMode,
    },
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
    /// Fee estimate for an arbitrary EVM payload (DEX approve/swap confirm card).
    EstimateEvmFee {
        tx: EvmTransaction,
    },
    /// After a Dex approve broadcast: wait for allowance, then estimate swap gas.
    DexSwapEstimateAfterApprove {
        rpc_url: String,
        token_in: String,
        owner: String,
        router: String,
        amount_in: String,
        tx: EvmTransaction,
    },
    /// Read-only: does the router already have enough ERC-20 allowance for this swap?
    DexAllowanceCheck {
        rpc_url: String,
        token_in: String,
        owner: String,
        router: String,
        amount_in: String,
    },
    /// EVM call with a user-approved fee (matches Send confirm UX).
    SendEvmWithFee {
        tx: EvmTransaction,
        fee: Fee,
    },
    /// Single-aggregator quote (no signing).
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
    /// Parallel quotes from every live aggregator (no signing).
    AggCompareQuote {
        token_in: String,
        token_out: String,
        amount: String,
        slippage: f64,
        native_in: bool,
        native_out: bool,
        account: Option<String>,
    },
    /// Direct DEX router quote (V2 getAmountsOut / V3 pool math).
    DexQuote {
        quote_gen: u64,
        chain_id: u64,
        rpc_url: String,
        protocol_v2: bool,
        router: String,
        /// Catalog QuoterV2 for V3 on mainnet venues (9mm 369); ignored on wiz4rd 943.
        quoter: Option<String>,
        amount_in: String,
        fee: u32,
        /// V2 hop addresses (from [`hop_tokens`]).
        path: Vec<String>,
        /// Exact tokens for V3 path resolution (direct pool preferred over WPLS hop).
        token_in: String,
        token_out: String,
        wpls: Option<String>,
        native_in: bool,
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
    /// Poll inclusion status for a broadcast hash (Send Done screen).
    PollTxStatus {
        tx_hash: String,
    },
    /// Refresh status for session recent broadcasts (hashes listed).
    RefreshBroadcastStatuses {
        hashes: Vec<String>,
    },
    /// Cancel or speed-up a pending session broadcast.
    ReplaceBroadcast {
        entry: BroadcastEntry,
        kind: ReplaceKind,
    },
    /// List V3 LP NFT positions for `owner` on a catalogued venue NPM.
    LpListPositions {
        venue: vaughan_core::core::DexVenue,
        chain_id: u64,
        rpc_url: String,
        owner: String,
    },
    /// List 9inch V2 LP pair balances for `owner`.
    LpListV2Positions {
        venue: vaughan_core::core::DexVenue,
        chain_id: u64,
        rpc_url: String,
        owner: String,
    },
    /// Build createPool, initialize, or mint for V3 add-liquidity.
    LpV3PoolDeployStep {
        venue: vaughan_core::core::DexVenue,
        chain_id: u64,
        rpc_url: String,
        from: String,
        token0: String,
        token1: String,
        fee: u32,
        dec0: u8,
        dec1: u8,
        pool_initial_price: String,
        pool_min_price: String,
        pool_max_price: String,
        amount0: String,
        amount1: String,
        /// Wait for a prior on-chain step before building the next tx.
        deploy_wait: vaughan_core::core::V3LpDeployWait,
        /// Label of the tx that was just broadcast (approve wait semantics).
        after_step_label: Option<String>,
    },
    /// Check NPM enable status for both tokens (PancakeSwap form step).
    LpEnableCheck {
        venue: vaughan_core::core::DexVenue,
        chain_id: u64,
        rpc_url: String,
        from: String,
        token0: String,
        token1: String,
        fee: u32,
        dec0: u8,
        dec1: u8,
        pool_initial_price: String,
        pool_min_price: String,
        pool_max_price: String,
        amount0: String,
        amount1: String,
    },
    /// Build the next Enable tx for the deposit form.
    LpEnablePrepare {
        venue: vaughan_core::core::DexVenue,
        chain_id: u64,
        rpc_url: String,
        from: String,
        token0: String,
        token1: String,
        fee: u32,
        dec0: u8,
        dec1: u8,
        pool_initial_price: String,
        pool_min_price: String,
        pool_max_price: String,
        amount0: String,
        amount1: String,
        symbol: String,
    },
    /// Wait for an Enable broadcast, then re-check allowances.
    LpEnableWait {
        venue: vaughan_core::core::DexVenue,
        chain_id: u64,
        rpc_url: String,
        from: String,
        token0: String,
        token1: String,
        fee: u32,
        dec0: u8,
        dec1: u8,
        pool_initial_price: String,
        pool_min_price: String,
        pool_max_price: String,
        amount0: String,
        amount1: String,
        after_step_label: String,
    },
    /// On-chain pool price + lifecycle for V3 add-LP deposit coupling.
    LpV3PoolQuote {
        venue: vaughan_core::core::DexVenue,
        chain_id: u64,
        rpc_url: String,
        token0: String,
        token1: String,
        fee: u32,
        dec0: u8,
        dec1: u8,
    },
    /// Deploy fixed-supply ERC-20 on testnet (meme coin launcher).
    DeployToken {
        name: String,
        symbol: String,
        supply: String,
    },
    /// MCP file-queue approve: sign + broadcast off the UI thread.
    McpQueuedApprove {
        proposal_id: String,
        source: String,
        proposal: vaughan_core::core::proposal::TxProposal,
        fee_override: Option<vaughan_core::chains::Fee>,
    },
}

/// Cached need-to-know strip shared across every unlocked screen.
#[derive(Debug, Clone, Default)]
pub struct ChromeSnapshot {
    pub balance: Option<Balance>,
    /// Suggested max fee / gas price in gwei (display string, e.g. `"12.4"`).
    pub gas_gwei: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
    /// Brief chrome toast (e.g. "F3 address copied") — shown under the address.
    pub flash: Option<String>,
    /// Optional title above [`Self::flash_table`] (LP Brew success summary).
    pub flash_title: Option<String>,
    /// Bordered verification table for post-action success (e.g. LP Brew mint).
    pub flash_table: Option<Vec<(String, String)>>,
    /// Ticks remaining before flash content clears (decremented each UI tick).
    pub flash_ticks_left: u8,
    /// Which status box is hotkeyed (F1 / F2 / F3).
    pub focus: ChromeFocus,
    /// F2 ↑/↓ asset cycle. Filled by [`UiJob::RefreshAssets`] — same set as
    /// [`WalletState::assets`] (includes user-imported tokens at zero balance).
    pub assets: Vec<Balance>,
    pub asset_idx: usize,
    pub assets_loading: bool,
    /// Pending F1 network list index (↑/↓ preview; Enter commits).
    pub pending_network_idx: Option<usize>,
    /// Pending F2 asset index.
    pub pending_asset_idx: Option<usize>,
    /// After a swap/send, select this ERC-20 in F2 once [`RefreshAssets`] completes.
    pub pending_asset_address: Option<String>,
    /// Pending F3 account index (`Account::index`).
    pub pending_account_index: Option<u32>,
    /// Count of MCP proposals waiting in the file queue.
    pub mcp_pending: usize,
    /// Loopback MCP listener (Cursor / Claude agents) — shown on F1 network strip.
    pub mcp_listener: crate::mcp::McpListenerState,
}

/// Status-strip focus for F1 / F2 / F3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChromeFocus {
    #[default]
    None,
    /// F1 — cycle networks with ↑/↓, Enter to set.
    Network,
    /// F2 — cycle assets with ↑/↓, Enter to set.
    Asset,
    /// F3 — cycle accounts with ↑/↓, Enter to set.
    Account,
}

/// Build the F2 asset cycle from a [`WalletState::assets`] fetch.
///
/// Core already drops zero-balance curated/discovered tokens but keeps
/// user-imported customs even at zero — do not re-filter here.
pub fn chrome_assets_from_fetch(assets: Vec<Balance>) -> Vec<Balance> {
    assets
}

/// Index in the F2 asset cycle for `address` (case-insensitive contract match).
pub fn asset_index_for_address(assets: &[Balance], address: &str) -> Option<usize> {
    let want = address.trim();
    if want.is_empty() {
        return None;
    }
    assets.iter().position(|b| {
        b.token
            .contract_address
            .as_ref()
            .is_some_and(|a| a.eq_ignore_ascii_case(want))
    })
}

/// Result delivered back to the UI thread via an unbounded channel.
pub enum UiJobResult {
    /// Vault unlock finished off the UI thread: derived accounts + the session
    /// mode the user picked at the unlock screen.
    Unlock(Result<UnlockCompletion, WalletError>),
    Chrome(Result<(Balance, String), WalletError>),
    Balance(Result<Balance, WalletError>),
    Assets(Result<Vec<Balance>, WalletError>),
    Fee(Result<Fee, WalletError>),
    /// Successful send includes a [`BroadcastReceipt`] for History tracking.
    Send(Result<BroadcastReceipt, WalletError>),
    SendStealth(Result<StealthSendResult, WalletError>),
    AggCompareQuote(Vec<vaughan_core::core::AggQuoteOutcome>),
    AggQuote(Result<vaughan_core::core::AggQuote, WalletError>),
    DexQuote {
        quote_gen: u64,
        result: Result<vaughan_core::core::DexQuote, WalletError>,
    },
    /// Dex F4 prep: true when router allowance already covers `amount_in`.
    DexAllowanceCheck(Result<bool, WalletError>),
    BridgeQuote(Box<Result<vaughan_core::core::BridgeQuote, WalletError>>),
    Activity(Result<Vec<vaughan_core::chains::TxRecord>, WalletError>),
    Allowances(Result<Vec<vaughan_core::chains::AllowanceEntry>, WalletError>),
    TxStatus(Result<vaughan_core::chains::TxStatus, WalletError>),
    /// Updated statuses for session broadcasts `(hash, status)`.
    BroadcastStatuses(Result<Vec<(String, vaughan_core::chains::TxStatus)>, WalletError>),
    /// V3 LP positions from NPM scan.
    LpPositions(Result<Vec<vaughan_core::core::V3PositionInfo>, WalletError>),
    /// V2 LP positions (9inch pair balances).
    LpV2Positions(Result<Vec<vaughan_core::core::V2LpPosition>, WalletError>),
    /// Prepared V3 createPool or initialize transaction + confirm label.
    LpV3PoolDeployStep(Result<(vaughan_core::chains::EvmTransaction, String), WalletError>),
    LpEnableCheck(Result<Option<(bool, bool)>, WalletError>),
    LpEnablePrepare(Result<(vaughan_core::chains::EvmTransaction, String, String), WalletError>),
    LpEnableWait(Result<(), WalletError>),
    /// Live pool sqrt/tick for V3 deposit preview.
    LpV3PoolQuote(Result<vaughan_core::core::V3LpPoolQuote, WalletError>),
    /// Meme-coin deploy + auto-import.
    DeployToken(Result<vaughan_core::core::TokenLaunchOutcome, WalletError>),
    /// MCP file-queue proposal executed (tx hash hex + proposal for success flash).
    McpQueuedApprove {
        result: Result<String, WalletError>,
        proposal: vaughan_core::core::proposal::TxProposal,
        proposal_id: String,
    },
}

/// Successful off-thread unlock: derived accounts plus the session mode picked
/// at the unlock screen (applied to the wallet by the app on completion).
pub struct UnlockCompletion {
    pub accounts: AccountManager,
    pub mode: OperatingMode,
}

/// Frames for a braille spinner (no extra deps).
pub fn spinner_frame(tick: u64) -> char {
    const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[(tick as usize) % FRAMES.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use vaughan_core::chains::{Balance, TokenInfo};

    #[test]
    fn asset_index_for_address_matches_contract() {
        let assets = vec![
            Balance {
                token: TokenInfo {
                    symbol: "PLS".into(),
                    name: "PulseChain".into(),
                    decimals: 18,
                    contract_address: None,
                },
                raw: "1".into(),
                formatted: "1".into(),
                usd_value: None,
            },
            Balance {
                token: TokenInfo {
                    symbol: "WZRD".into(),
                    name: "Wizard".into(),
                    decimals: 18,
                    contract_address: Some("0x29bab93456c0E97EE931C1554c7C215480aa7766".into()),
                },
                raw: "670201331000844945".into(),
                formatted: "0.67".into(),
                usd_value: None,
            },
        ];
        assert_eq!(
            asset_index_for_address(&assets, "0x29bab93456c0e97ee931c1554c7c215480aa7766"),
            Some(1)
        );
        assert_eq!(asset_index_for_address(&assets, "0xdead"), None);
    }

    #[test]
    fn chrome_assets_from_fetch_keeps_zero_balance_imports() {
        let assets = vec![
            Balance {
                token: TokenInfo {
                    symbol: "PLS".into(),
                    name: "PulseChain".into(),
                    decimals: 18,
                    contract_address: None,
                },
                raw: "1000000000000000000".into(),
                formatted: "1".into(),
                usd_value: None,
            },
            Balance {
                token: TokenInfo {
                    symbol: "MEME".into(),
                    name: "Meme".into(),
                    decimals: 18,
                    contract_address: Some("0x2222222222222222222222222222222222222222".into()),
                },
                raw: "0".into(),
                formatted: "0".into(),
                usd_value: None,
            },
        ];
        let out = chrome_assets_from_fetch(assets);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].token.symbol, "MEME");
    }
}
