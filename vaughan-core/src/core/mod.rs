//! Core wallet services: persistence, accounts, network, transactions, and the
//! top-level [`WalletState`] that ties them together for the UI.

pub mod account;
pub mod agent_autonomy;
pub mod aggregator;
pub mod bridge;
pub mod broadcasts;
pub mod dex_catalog;
pub mod dex_lp;
pub mod dex_quote;
pub mod dex_routers;
pub mod lp_brew;
pub mod lp_deploy;
pub mod lp_smoke;
pub mod mcp_host;
pub mod mcp_ipc;
pub mod network;
pub mod persistence;
pub mod piteas;
pub mod profile;
pub mod proposal;
pub mod proposal_verify;
pub mod provider_session;
pub mod site_grants;
pub mod stealth;
pub mod token_launch;
pub mod transaction;
pub mod v2_lp;
pub mod vault_secrets;
pub mod vb_browser;
pub mod vb_cdp;
pub mod wallet;
pub mod wiz4rd;

pub use account::{Account, AccountManager, IMPORTED_INDEX_BASE};
pub use agent_autonomy::{
    operator_connect_allow_suffixes, operator_connect_allowed, AgentAutonomyTier,
};
pub use aggregator::{
    assert_agg_exec_targets, is_allowed_agg_router, quote_aggregator, quote_live_aggregators,
    rank_agg_quote_outcomes, AggAccess, AggExecTx, AggQuote, AggQuoteOutcome, AggQuoteRequest,
    AggVenue, SquirrelPreview, SquirrelSwapClient, AGG_VENUES, OFFICIAL_AGG_ROUTERS,
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
pub use dex_catalog::{
    chain_label, default_lp_v3_venue, default_lp_venue, lp_stack_for_chain, lp_v2_venue,
    lp_v3_venue_picker, lp_v3_venues, missing_router_hint, parse_dex_venue_label,
    venue_pool_deployer, venue_position_manager, venue_quoter_v2, venue_slug, venue_swap_router, venue_v2_factory,
    venue_v3_factory, DexContractRole, DexProtocol, DexVenue, LpStack, DEX_VENUES,
};
pub use dex_lp::{
    build_v3_collect_evm, build_v3_create_pool_evm, build_v3_decrease_evm, build_v3_increase_evm,
    build_v3_initialize_pool_evm, build_v3_initialize_pool_from_human_price_evm,
    build_v3_initialize_pool_from_tick_evm, build_v3_mint_evm, default_full_range_ticks,
    discover_v3_pool_fee_tier, display_price_range_from_preset, fetch_v3_lp_pool_quote,
    is_lp_rpc_transport, list_v3_lp_positions, load_v3_lp_pool, merge_rpc_urls,
    sqrt_price_x96_from_tick, v3_initial_tick_from_human_price, v3_lp_build_next_enable_tx,
    v3_lp_prepare_deploy_step, v3_lp_run_deploy_wait, v3_lp_sdk_config, v3_lp_token_enable_status,
    lp_deploy_fixup_swapped_amounts, v3_lp_deploy_mint_amounts, v3_lp_mint_tick_range, v3_pool_lifecycle, v3_pool_sqrt_u160,     v3_preview_mint_deposits_from_amount0, v3_preview_mint_deposits_from_amount0_ticks,
    v3_preview_mint_deposits_from_amount1, v3_preview_mint_deposits_from_amount1_ticks,
    v3_range_ticks_from_human_prices,
    v3_sqrt_and_tick_for_preview, with_lp_rpc_urls, wiz4rd_sdk_config, V3LpDeployContext,
    V3LpDeployParams, V3LpDeployWait, V3LpPoolQuote, V3PoolLifecycle, V3PositionInfo,
    V3_LP_FEE_TIERS,
};
pub use dex_quote::{
    discover_v3_swap_route, encode_v3_packed_path, erc20_allowance_covers, min_out_after_slippage,
    quote_v2_exact_in, quote_v3_exact_in, quote_v3_path_exact_in, resolve_v3_swap_path,
    wait_erc20_allowance, DexQuote, V3DiscoveredRoute, DEFAULT_DEX_SLIPPAGE_BPS,
};
pub use dex_routers::{
    dex_routers_labeled, is_allowed_dex_router, wpls_for_chain, PULSEX_V2_MAINNET,
};
pub use lp_brew::{
    load_brew_file, lp_human_inputs_to_deploy_params, pool_price_to_user_price,
    resolve_lp_brew_fee, resolve_lp_brew_token, sort_lp_token_pair, trim_float_string,
    user_price_range_to_pool_prices, user_price_to_pool_price, LpDeployBrewFile, LpHumanInputs,
    LpRangeInput, SortedLpTokens,
};
pub use lp_deploy::{
    build_lp_deploy_batch_calls, lp_deploy_advance_after_broadcast, lp_deploy_job_create,
    lp_deploy_job_load, lp_deploy_job_mark_done, lp_deploy_job_save, lp_deploy_next_step,
    lp_deploy_plan, lp_deploy_preflight, lp_deploy_estimate_gas_limit, lp_deploy_retry_after_approve,
    lp_deploy_step_to_proposal, lp_deploy_wallet_gas_limit, wait_after_label,
    LpDeployBatchPlan, LpDeployJob, LpDeployJobStatus, LpDeployPlan, LpDeployStepOutcome,
    StoredLpDeployParams,
};
pub use lp_smoke::{LpSmoke943Pair, LP_SMOKE_943, LP_SMOKE_943_VENUE, RPC_943};
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
    reject_deferred_sentient_profile, sentient_mode_enabled,
    merge_default_trusted_dapps, trusted_dapp_allow_hosts, CustomNetwork, CustomToken,
    PersistedState, ProfileMeta, StateManager, TrustedDapp, DEFAULT_PROFILE, DEGEN_PROFILE,
    SENTIENT_PROFILE,
};
pub use piteas::{
    AuthStyle, MethodParameters, NativeToken, PiteasClient, PiteasFileConfig, PiteasQuote,
    QuoteRequest, PITEAS_ROUTER_MAINNET,
};
pub use profile::{tui_mode_for_profile, OperatingMode};
pub use proposal_verify::{
    lp_deploy_mint_success_rows, lp_deploy_step_verify_rows, lp_deploy_step_verify_title,
    npm_mint_token_id_for_tx, npm_mint_token_id_from_logs, short_address, short_tx_hash,
    VerifyRow,
};
pub use proposal::{
    apply_proposal, fee_spike_exceeds_threshold, guard_mainnet_write, mcp_control_port,
    mcp_mainnet_writes_allowed, proposal_status_json, validate_proposal_id, McpSessionToken,
    ProposalError, ProposalQueue, ProposalStatus, ProposalType, QueuedProposal, TxProposal,
    MAX_PENDING_PROPOSALS, MAX_PROPOSAL_ID_LEN, MCP_CONTROL_PORT, MCP_ENQUEUE_RATE_WINDOW_SECS,
    MCP_FEE_SPIKE_THRESHOLD_BPS, MCP_MAX_ENQUEUES_PER_WINDOW, PROPOSAL_TTL_SECS,
};
pub use provider_session::{ProviderSessionToken, PROVIDER_SESSION_FILE};
pub use stealth::{looks_like_stealth_uri, StealthNote, StealthSendResult};
pub use token_launch::{
    build_erc20_deploy_evm, encode_erc20_deploy_calldata, parse_token_supply_human,
    token_launch_allowed, validate_token_name, validate_token_symbol, TokenLaunchOutcome,
    TOKEN_LAUNCH_DECIMALS,
};
pub use transaction::{
    format_base_units, format_display_amount, parse_native_amount, TransactionService,
};
pub use v2_lp::{
    build_v2_add_liquidity_evm, build_v2_remove_liquidity_evm, default_v2_watch_pairs,
    get_v2_pair_address, list_v2_lp_positions, V2LpPosition,
};
pub use wallet::{ChromeRpcSnapshot, NetworkRpcSnapshot, UnlockPayload, WalletState};
pub use wiz4rd::{
    deployment_for_chain, position_manager as wiz4rd_position_manager,
    swap_router as wiz4rd_swap_router, Wiz4rdDeployment, DEPLOYMENT_943, WIZ4RD_FEE_TIERS,
};
