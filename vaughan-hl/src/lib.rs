//! `vaughan-hl` — Modular Hyperliquid multi-indicator strategy lab, deterministic
//! `R:R` replay backtester, L1 Phantom-Agent signer, encrypted agent keystore,
//! and closed-candle execution engine for `Vaughan-CLI`.

pub mod engine;
pub mod error;
pub mod indicators;
pub mod keystore;
pub mod live;
pub mod market_data;
pub mod replay;
pub mod siphon;
pub mod strategy;
#[cfg(feature = "tui")]
pub mod tui;
pub mod wire;
