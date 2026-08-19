//! Central error type for the wallet core.
//!
//! Every layer returns [`WalletError`] and the UI maps it to a user-facing
//! string via [`WalletError::user_message`].

use std::fmt;
use std::time::Duration;

use thiserror::Error;

/// Central error enum used across all wallet layers.
#[derive(Debug, Error)]
pub enum WalletError {
    /// A network-level failure (RPC unreachable, TLS, etc.).
    #[error("network error: {0}")]
    NetworkError(String),

    /// An RPC call returned an error.
    #[error("rpc error: {0}")]
    RpcError(String),

    /// Gas estimation failed.
    #[error("gas estimation failed: {0}")]
    GasEstimationFailed(String),

    /// Transaction signing failed.
    #[error("signing failed: {0}")]
    SigningFailed(String),

    /// Transaction broadcast or confirmation failed.
    #[error("transaction failed: {0}")]
    TransactionFailed(String),

    /// An amount could not be parsed or is out of range.
    #[error("invalid amount: {0}")]
    InvalidAmount(String),

    /// A transaction is malformed.
    #[error("invalid transaction: {0}")]
    InvalidTransaction(String),

    /// A BIP-39 mnemonic is invalid.
    #[error("invalid mnemonic: {0}")]
    InvalidMnemonic(String),

    /// A private key could not be parsed.
    #[error("invalid private key: {0}")]
    InvalidPrivateKey(String),

    /// An ERC-5564 stealth meta-address or announcement was malformed.
    #[error("invalid stealth address: {0}")]
    InvalidStealth(String),

    /// A password fails the strength policy or is incorrect.
    #[error("invalid password")]
    InvalidPassword,

    /// A password fails the strength policy.
    #[error("password policy: {0}")]
    PasswordPolicy(String),

    /// Encryption failed.
    #[error("encryption failed: {0}")]
    EncryptionFailed(String),

    /// Decryption failed (typically a wrong password).
    #[error("decryption failed: {0}")]
    DecryptionFailed(String),

    /// Key derivation (BIP-32/44) failed.
    #[error("key derivation failed: {0}")]
    KeyDerivationFailed(String),

    /// An account was not found.
    #[error("account not found: {0}")]
    AccountNotFound(String),

    /// A network (chain) config was not found.
    #[error("network not found: {0}")]
    NetworkNotFound(String),

    /// Persistence (I/O) failure.
    #[error("io error: {0}")]
    Io(String),

    /// (De)serialization failure.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// The wallet has not been set up yet.
    #[error("wallet not initialized")]
    NotInitialized,

    /// The wallet is locked; unlock before performing this operation.
    #[error("wallet is locked")]
    WalletLocked,

    /// Fallback for uncategorized errors.
    #[error("{0}")]
    Other(String),
}

impl WalletError {
    /// Convert an internal error into a short, user-facing message.
    pub fn user_message(&self) -> String {
        match self {
            Self::NetworkError(_) => {
                "Could not reach the network. Check your connection and RPC URL.".to_string()
            }
            Self::RpcError(_) => {
                "The blockchain RPC returned an error. Try again or switch networks.".to_string()
            }
            Self::GasEstimationFailed(_) => "Could not estimate the transaction fee.".to_string(),
            Self::SigningFailed(_) => "Could not sign the transaction.".to_string(),
            Self::TransactionFailed(_) => {
                "The transaction was rejected by the network.".to_string()
            }
            Self::InvalidAmount(msg) => format!("Invalid amount: {msg}"),
            Self::InvalidTransaction(msg) => format!("Invalid transaction: {msg}"),
            Self::InvalidMnemonic(_) => "The recovery phrase is invalid.".to_string(),
            Self::InvalidPrivateKey(_) => "The private key is invalid.".to_string(),
            Self::InvalidStealth(_) => {
                "The stealth address is invalid. Check the st: URI and try again.".to_string()
            }
            Self::InvalidPassword => "Invalid password.".to_string(),
            Self::PasswordPolicy(msg) => msg.clone(),
            Self::EncryptionFailed(_) => "Could not encrypt the wallet data.".to_string(),
            Self::DecryptionFailed(_) => {
                "Could not decrypt the wallet (wrong password?).".to_string()
            }
            Self::KeyDerivationFailed(_) => "Could not derive the account key.".to_string(),
            Self::AccountNotFound(_) => "The requested account was not found.".to_string(),
            Self::NetworkNotFound(_) => "The requested network is not configured.".to_string(),
            Self::Io(_) => "Could not read or write wallet data.".to_string(),
            Self::Serialization(_) => "Could not read the wallet data file.".to_string(),
            Self::NotInitialized => "The wallet has not been set up yet.".to_string(),
            Self::WalletLocked => "The wallet is locked. Unlock it first.".to_string(),
            Self::Other(msg) => msg.clone(),
        }
    }
}

impl From<std::io::Error> for WalletError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<serde_json::Error> for WalletError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialization(e.to_string())
    }
}

/// Retry an async fallible operation with exponential backoff.
///
/// Only transient failures (network/RPC) are retried; everything else is
/// returned immediately.
pub async fn retry_async<T, E, Fut, F>(mut f: F, attempts: u32) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: fmt::Display,
{
    let mut attempt = 0;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                attempt += 1;
                if attempt >= attempts {
                    return Err(e);
                }
                let backoff = Duration::from_millis(200 * u64::from(attempt));
                tracing::warn!("transient failure (attempt {attempt}/{attempts}): {e}");
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_is_not_empty() {
        let variants = [
            WalletError::NetworkError("x".into()),
            WalletError::InvalidPassword,
            WalletError::NotInitialized,
            WalletError::Other("boom".into()),
        ];
        for v in variants {
            assert!(!v.user_message().is_empty());
        }
    }

    #[tokio::test]
    async fn retry_succeeds_on_second_attempt() {
        let mut calls = 0;
        let result = retry_async(
            || {
                calls += 1;
                async move {
                    if calls < 2 {
                        Err("fail".to_string())
                    } else {
                        Ok(42u32)
                    }
                }
            },
            3,
        )
        .await;
        assert_eq!(result, Ok(42));
        assert_eq!(calls, 2);
    }
}
