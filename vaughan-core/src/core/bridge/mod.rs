//! LibertySwap cross-chain bridge client (Pulse-centered).
//!
//! Public quote API (no partner key):
//! `GET https://apis.libertyswap.finance/v3/swap/quote`
//!
//! Docs lag the live path (`/v3/swap/quote`, recipient required). Vaughan
//! targets the host the web app uses. Railgun / privacy routes are out of
//! scope. See `docs/bridge.md`.

mod client;
mod types;

pub use client::{
    assert_bridge_exec_targets, is_whitelisted_router, LibertySwapClient, LIBERTY_SWAP_V3_BASE,
    OFFICIAL_ROUTERS,
};
pub use types::{
    BridgeApproval, BridgeAsset, BridgeChainPreset, BridgeExecTx, BridgeFee, BridgeQuote,
    BridgeQuoteRequest, BridgeTokenInfo, BRIDGE_CHAIN_PRESETS,
};
