//! Error types for `vaughan-hl`.

use thiserror::Error;

/// Unified error type for strategy validation, market data, indicators, replay, L1 wire signing, and keystore.
#[derive(Debug, Error)]
pub enum HlError {
    #[error("invalid strategy configuration: {0}")]
    InvalidStrategy(String),

    #[error("risk guard violation: {0}")]
    RiskViolation(String),

    #[error(
        "insufficient candle history: have {have} closed bars, need at least {need} for warm-up"
    )]
    InsufficientHistory { have: usize, need: usize },

    #[error("market data error: {0}")]
    MarketData(String),

    #[error("L1 wire format error: {0}")]
    Wire(String),

    #[error("EIP-712 signing error: {0}")]
    Signing(String),

    #[error("agent keystore error: {0}")]
    Keystore(String),

    #[error("Hyperliquid API error: {0}")]
    Api(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
}
