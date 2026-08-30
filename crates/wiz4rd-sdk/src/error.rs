//! Error types for wiz4rd-sdk.

use alloy::sol_types::Error as SolError;
use alloy::transports::TransportError;

/// Errors produced by the SDK layer.
#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("config: {0}")]
    Config(String),

    /// Required contract address missing from config (e.g. `swap_router`).
    #[error("config missing required address: {0}")]
    MissingAddress(&'static str),

    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("url: {0}")]
    Url(#[from] url::ParseError),

    #[error("rpc: {0}")]
    Rpc(#[from] TransportError),

    #[error("math: {0}")]
    Math(String),

    /// A view call returned data that failed ABI decoding.
    #[error("decode: {0}")]
    Decode(#[from] SolError),

    /// No pool deployed for this token pair and fee tier (`getPool` returned zero).
    #[error("pool not found")]
    PoolNotFound,
}

pub type SdkResult<T> = Result<T, SdkError>;
