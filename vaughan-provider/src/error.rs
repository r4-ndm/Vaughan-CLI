//! EIP-1193 error codes and the provider's central error type.
//!
//! JSON-RPC failures carry a numeric code plus a short message; EIP-1193
//! extends that with its own code range (4001–4999) for user-facing
//! conditions such as "user rejected the request" and "chain disconnected".
//! [`ProviderError`] maps internal failures onto those codes so every layer
//! (transport, JSON-RPC framing, method handlers) speaks the same wire
//! language. Sensitive material must never end up in these messages.

/// EIP-1193 error codes (https://eips.ethereum.org/EIPS/eip-1193#provider-errors).
pub mod codes {
    /// The user rejected the request (e.g. denied a sign prompt).
    pub const USER_REJECTED: i64 = 4001;
    /// The request is not authorized for the calling origin.
    pub const UNAUTHORIZED: i64 = 4100;
    /// The provider does not support the requested method.
    pub const UNSUPPORTED_METHOD: i64 = 4200;
    /// The provider is disconnected from all chains.
    pub const DISCONNECTED: i64 = 4900;
    /// The provider is disconnected from the specified chain.
    pub const CHAIN_DISCONNECTED: i64 = 4901;
    /// The requested chain is not recognized (EIP-3326 `wallet_switchEthereumChain`).
    pub const UNRECOGNIZED_CHAIN: i64 = 4902;

    // JSON-RPC 2.0 standard errors.
    /// Invalid JSON was received by the server.
    pub const PARSE_ERROR: i64 = -32700;
    /// The JSON sent is not a valid Request object.
    pub const INVALID_REQUEST: i64 = -32600;
    /// The method does not exist / is not implemented.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// Invalid method parameter(s).
    pub const INVALID_PARAMS: i64 = -32602;
    /// Internal JSON-RPC error.
    pub const INTERNAL_ERROR: i64 = -32603;
}

/// Central error type for the provider bridge.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ProviderError {
    /// The user denied an approval prompt (EIP-1193 4001).
    #[error("user rejected the request")]
    UserRejected,

    /// The calling origin is not authorized for this request (4100).
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// The provider does not implement the requested method (4200).
    #[error("unsupported method: {0}")]
    UnsupportedMethod(String),

    /// The provider is disconnected from the network (4900).
    #[error("disconnected: {0}")]
    Disconnected(String),

    /// The provider is disconnected from the requested chain (4901).
    #[error("chain disconnected: {0}")]
    ChainDisconnected(String),

    /// The requested chain id is not one of the wallet's networks (4902).
    #[error("unrecognized chain: {0}")]
    UnrecognizedChain(String),

    /// A request had malformed parameters (-32602).
    #[error("invalid params: {0}")]
    InvalidParams(String),

    /// An unexpected internal failure (-32603).
    #[error("internal error: {0}")]
    Internal(String),

    /// A transport-level failure (socket, handshake, framing).
    #[error("transport error: {0}")]
    Transport(String),
}

impl ProviderError {
    /// The JSON-RPC / EIP-1193 error code for this failure.
    pub fn code(&self) -> i64 {
        match self {
            Self::UserRejected => codes::USER_REJECTED,
            Self::Unauthorized(_) => codes::UNAUTHORIZED,
            Self::UnsupportedMethod(_) => codes::UNSUPPORTED_METHOD,
            Self::Disconnected(_) => codes::DISCONNECTED,
            Self::ChainDisconnected(_) => codes::CHAIN_DISCONNECTED,
            Self::UnrecognizedChain(_) => codes::UNRECOGNIZED_CHAIN,
            Self::InvalidParams(_) => codes::INVALID_PARAMS,
            Self::Internal(_) | Self::Transport(_) => codes::INTERNAL_ERROR,
        }
    }
}

impl From<std::io::Error> for ProviderError {
    fn from(e: std::io::Error) -> Self {
        Self::Transport(e.to_string())
    }
}

impl From<serde_json::Error> for ProviderError {
    fn from(e: serde_json::Error) -> Self {
        Self::InvalidParams(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_eip_1193() {
        assert_eq!(codes::USER_REJECTED, 4001);
        assert_eq!(codes::UNAUTHORIZED, 4100);
        assert_eq!(codes::UNSUPPORTED_METHOD, 4200);
        assert_eq!(codes::DISCONNECTED, 4900);
        assert_eq!(codes::CHAIN_DISCONNECTED, 4901);
    }

    #[test]
    fn each_variant_maps_to_a_code() {
        let cases = [
            (ProviderError::UserRejected, 4001),
            (ProviderError::Unauthorized("x".into()), 4100),
            (ProviderError::UnsupportedMethod("x".into()), 4200),
            (ProviderError::Disconnected("x".into()), 4900),
            (ProviderError::ChainDisconnected("x".into()), 4901),
            (ProviderError::UnrecognizedChain("x".into()), 4902),
            (ProviderError::InvalidParams("x".into()), -32602),
            (ProviderError::Internal("x".into()), -32603),
            (ProviderError::Transport("x".into()), -32603),
        ];
        for (err, expected) in cases {
            assert_eq!(err.code(), expected, "wrong code for {err:?}");
        }
    }

    #[test]
    fn io_and_json_errors_convert() {
        let io = std::io::Error::new(std::io::ErrorKind::Other, "boom");
        assert!(matches!(
            ProviderError::from(io),
            ProviderError::Transport(_)
        ));
        let json = serde_json::from_str::<serde_json::Value>("nope").unwrap_err();
        assert!(matches!(
            ProviderError::from(json),
            ProviderError::InvalidParams(_)
        ));
    }
}
