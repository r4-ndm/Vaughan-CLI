//! Core wallet services: persistence, accounts, network, transactions, and the
//! top-level [`WalletState`] that ties them together for the UI.

pub mod account;
pub mod aggregator;
pub mod bridge;
pub mod broadcasts;
pub mod dex_quote;
pub mod dex_routers;
pub mod mcp_host;
pub mod mcp_ipc;
pub mod network;
pub mod persistence;
pub mod piteas;
pub mod profile;
pub mod proposal;
pub mod provider_session;
pub mod site_grants;
pub mod stealth;
pub mod transaction;
pub mod vault_secrets;
pub mod vb_browser;
pub mod vb_cdp;
pub mod wallet;
pub mod wiz4rd;

pub use account::{Account, AccountManager, IMPORTED_INDEX_BASE};
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
pub use broadcasts::{
    mark_replaced, push_recent, BroadcastEntry, BroadcastReceipt, ReplaceKind,
    MAX_RECENT_BROADCASTS,
};
pub use dex_quote::{
    min_out_after_slippage, quote_v2_exact_in, quote_v3_exact_in, DexQuote,
    DEFAULT_DEX_SLIPPAGE_BPS,
};
pub use dex_routers::{
    dex_routers_labeled, is_allowed_dex_router, wpls_for_chain, PULSEX_V2_MAINNET,
};
pub use mcp_host::{
    dispatch_ipc_request, handle_ipc_connection, read_ipc_line, McpHostBackend, McpProposeOutcome,
    McpSessionData,
};
pub use mcp_ipc::{
    decode_ipc_line, decode_line, encode_line, session_token_valid, McpIpcError, McpIpcLineError,
    McpIpcRequest, McpIpcResponse, MCP_IPC_MAX_LINE_BYTES,
};
pub use network::NetworkService;
pub use persistence::{
    default_ipfs_gateway_hosts, default_trusted_dapps, is_sentient_profile,
    merge_default_trusted_dapps, trusted_dapp_allow_hosts, CustomNetwork, CustomToken,
    PersistedState, StateManager, TrustedDapp, DEFAULT_PROFILE, DEGEN_PROFILE, SENTIENT_PROFILE,
};
pub use piteas::{
    AuthStyle, MethodParameters, NativeToken, PiteasClient, PiteasFileConfig, PiteasQuote,
    QuoteRequest, PITEAS_ROUTER_MAINNET,
};
pub use profile::OperatingMode;
pub use proposal::{
    apply_proposal, fee_spike_exceeds_threshold, guard_mainnet_write, mcp_control_port,
    mcp_mainnet_writes_allowed, proposal_status_json, validate_proposal_id, McpSessionToken,
    ProposalError, ProposalQueue, ProposalStatus, ProposalType, QueuedProposal, TxProposal,
    MAX_PENDING_PROPOSALS, MAX_PROPOSAL_ID_LEN, MCP_CONTROL_PORT, MCP_ENQUEUE_RATE_WINDOW_SECS,
    MCP_FEE_SPIKE_THRESHOLD_BPS, MCP_MAX_ENQUEUES_PER_WINDOW, PROPOSAL_TTL_SECS,
};
pub use provider_session::{ProviderSessionToken, PROVIDER_SESSION_FILE};
pub use stealth::{looks_like_stealth_uri, StealthNote, StealthSendResult};
pub use transaction::{
    format_base_units, format_display_amount, parse_native_amount, TransactionService,
};
pub use wallet::{ChromeRpcSnapshot, NetworkRpcSnapshot, WalletState};
pub use wiz4rd::{
    deployment_for_chain, position_manager as wiz4rd_position_manager,
    swap_router as wiz4rd_swap_router, Wiz4rdDeployment, DEPLOYMENT_943, WIZ4RD_FEE_TIERS,
};
