//! Core wallet services: persistence, accounts, network, transactions, and the
//! top-level [`WalletState`] that ties them together for the UI.

pub mod account;
pub mod aggregator;
pub mod bridge;
pub mod dex_routers;
pub mod network;
pub mod persistence;
pub mod piteas;
pub mod profile;
pub mod stealth;
pub mod transaction;
pub mod vault_secrets;
pub mod wallet;

pub use account::{Account, AccountManager};
pub use aggregator::{
    assert_agg_exec_targets, is_allowed_agg_router, quote_aggregator, AggAccess, AggExecTx,
    AggQuote, AggQuoteRequest, AggVenue, SquirrelPreview, SquirrelSwapClient, AGG_VENUES,
    OFFICIAL_AGG_ROUTERS,
};
pub use bridge::{
    assert_bridge_exec_targets, is_whitelisted_router, BridgeApproval, BridgeAsset,
    BridgeChainPreset, BridgeExecTx, BridgeFee, BridgeQuote, BridgeQuoteRequest, BridgeTokenInfo,
    LibertySwapClient, BRIDGE_CHAIN_PRESETS, LIBERTY_SWAP_V3_BASE, OFFICIAL_ROUTERS,
};
pub use dex_routers::{is_allowed_dex_router, PULSEX_V2_MAINNET};
pub use network::NetworkService;
pub use persistence::{
    default_trusted_dapps, merge_default_trusted_dapps, CustomNetwork, CustomToken, PersistedState,
    StateManager, TrustedDapp, DEFAULT_PROFILE, DEGEN_PROFILE,
};
pub use piteas::{
    AuthStyle, MethodParameters, NativeToken, PiteasClient, PiteasFileConfig, PiteasQuote,
    QuoteRequest, PITEAS_ROUTER_MAINNET,
};
pub use profile::OperatingMode;
pub use stealth::{looks_like_stealth_uri, StealthNote, StealthSendResult};
pub use transaction::{format_base_units, parse_native_amount, TransactionService};
pub use wallet::{ChromeRpcSnapshot, WalletState};
