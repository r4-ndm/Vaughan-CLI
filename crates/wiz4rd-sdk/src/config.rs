//! Chain + contract configuration for wiz4rd-sdk.
//!
//! Loaded from a TOML file (default `wiz4rd.toml` in the current dir, or
//! `$WIZ4RD_CONFIG`), with env-var overrides for the RPC endpoint and chain.
//! See `config.example.toml` for the schema.
//!
//! Contract addresses are **optional** on purpose: before Phase 3 deploys they
//! are unknown. The pool reader resolves `factory` / `poolDeployer` from the
//! SwapRouter at runtime; only addresses the SDK must target directly
//! (`swap_router`, `position_manager`) are config fields.

use std::path::Path;

use alloy::primitives::Address;
use serde::Deserialize;

use crate::error::{SdkError, SdkResult};

/// RPC endpoints per chain, matching `docs/addresses.md`.
pub mod rpc {
    pub const PULSECHAIN_MAINNET: &str = "https://rpc.pulsechain.com";
    pub const PULSECHAIN_TESTNET_V4: &str = "https://rpc.v4.testnet.pulsechain.com";
}

/// Chain ids (mirrors `wiz4rd_sdk::tokens::chain`).
pub mod chain {
    pub const PULSECHAIN_MAINNET: u64 = 369;
    pub const PULSECHAIN_TESTNET_V4: u64 = 943;
}

/// Configuration for one wiz4rd deployment.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// JSON-RPC endpoint. Defaults per `chain_id` if unset.
    pub rpc_url: Option<String>,
    /// Chain id (369 mainnet / 943 testnet V4).
    pub chain_id: u64,
    /// PancakeV3Factory address (core). Required for pool reads (getPool).
    pub factory: Option<Address>,
    /// SwapRouter contract address (periphery). Required for swap tx builders.
    pub swap_router: Option<Address>,
    /// NonfungiblePositionManager address (periphery). Required for
    /// liquidity/position operations.
    pub position_manager: Option<Address>,
    /// Default protocol fee (in hundredths of a bip, like Uniswap's
    /// `feeProtocol`). Explicitly 0 for now — a conscious default, not an
    /// accident. Set via `factory.setFeeProtocol` after deploy.
    pub protocol_fee: u32,
    /// Vaughan EIP-1193 provider server (`ws://127.0.0.1:8745`). When set,
    /// signing + broadcasting go through the running Vaughan wallet (TUI
    /// approval prompts) instead of the standalone keystore. Keys never leave
    /// Vaughan; wiz4rd polls its own RPC for the receipt.
    pub vaughan_provider: Option<String>,
    /// Origin header presented to Vaughan's trusted-origin allowlist (the
    /// value must be listed in Vaughan's `VAUGHAN_PROVIDER_TRUSTED_ORIGINS`).
    pub vaughan_origin: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rpc_url: None,
            chain_id: chain::PULSECHAIN_TESTNET_V4,
            factory: None,
            swap_router: None,
            position_manager: None,
            protocol_fee: 0,
            vaughan_provider: None,
            vaughan_origin: None,
        }
    }
}

impl Config {
    /// Load from a TOML file, then apply env-var overrides.
    ///
    /// Env vars: `WIZ4RD_CONFIG` (file path), `WIZ4RD_RPC_URL`,
    /// `WIZ4RD_CHAIN_ID`, `WIZ4RD_SWAP_ROUTER`, `WIZ4RD_POSITION_MANAGER`.
    pub fn load() -> SdkResult<Self> {
        let path = std::env::var("WIZ4RD_CONFIG").unwrap_or_else(|_| "wiz4rd.toml".into());
        Self::load_from(Path::new(&path))
    }

    /// Load from an explicit path; a missing file yields the default config
    /// (testnet) so the SDK still works before any deploy.
    pub fn load_from(path: &Path) -> SdkResult<Self> {
        let mut cfg = if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            toml::from_str(&raw)?
        } else {
            Config::default()
        };
        cfg.apply_env();
        Ok(cfg)
    }

    fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("WIZ4RD_RPC_URL") {
            self.rpc_url = Some(v);
        }
        if let Ok(v) = std::env::var("WIZ4RD_CHAIN_ID") {
            if let Ok(id) = v.parse() {
                self.chain_id = id;
            }
        }
        if let Ok(v) = std::env::var("WIZ4RD_FACTORY") {
            self.factory = v.parse().ok();
        }
        if let Ok(v) = std::env::var("WIZ4RD_SWAP_ROUTER") {
            self.swap_router = v.parse().ok();
        }
        if let Ok(v) = std::env::var("WIZ4RD_POSITION_MANAGER") {
            self.position_manager = v.parse().ok();
        }
        if let Ok(v) = std::env::var("WIZ4RD_VAUGHAN_PROVIDER") {
            if !v.is_empty() {
                self.vaughan_provider = Some(v);
            }
        }
        if let Ok(v) = std::env::var("WIZ4RD_VAUGHAN_ORIGIN") {
            if !v.is_empty() {
                self.vaughan_origin = Some(v);
            }
        }
    }

    /// Resolved RPC endpoint: explicit value, else the default for the chain.
    pub fn rpc_url(&self) -> &str {
        self.rpc_url.as_deref().unwrap_or(match self.chain_id {
            chain::PULSECHAIN_MAINNET => rpc::PULSECHAIN_MAINNET,
            _ => rpc::PULSECHAIN_TESTNET_V4,
        })
    }

    /// Build an alloy provider for this config's RPC endpoint.
    pub fn provider(&self) -> SdkResult<impl alloy::providers::Provider> {
        let url = self.rpc_url().parse().map_err(SdkError::Url)?;
        Ok(alloy::providers::ProviderBuilder::new().connect_http(url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_testnet_with_zero_protocol_fee() {
        let cfg = Config::default();
        assert_eq!(cfg.chain_id, chain::PULSECHAIN_TESTNET_V4);
        assert_eq!(cfg.protocol_fee, 0, "protocol fee is a conscious default");
        assert_eq!(cfg.rpc_url(), rpc::PULSECHAIN_TESTNET_V4);
    }

    #[test]
    fn missing_file_yields_default() {
        let cfg = Config::load_from(Path::new("/nonexistent/wiz4rd.toml")).unwrap();
        assert_eq!(cfg.chain_id, chain::PULSECHAIN_TESTNET_V4);
    }

    #[test]
    fn parses_toml_with_addresses() {
        let dir = std::env::temp_dir().join(format!("wiz4rd-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wiz4rd.toml");
        std::fs::write(
            &path,
            r#"
chain_id = 943
rpc_url = "https://rpc.v4.testnet.pulsechain.com"
swap_router = "0x1111111111111111111111111111111111111111"
position_manager = "0x2222222222222222222222222222222222222222"
protocol_fee = 0
"#,
        )
        .unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.chain_id, 943);
        assert_eq!(
            cfg.swap_router.unwrap(),
            "0x1111111111111111111111111111111111111111".parse::<Address>().unwrap()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn env_overrides_rpc() {
        std::env::set_var("WIZ4RD_RPC_URL", "https://example.invalid");
        let cfg = Config::default();
        std::env::remove_var("WIZ4RD_RPC_URL");
        assert_eq!(cfg.rpc_url(), "https://rpc.v4.testnet.pulsechain.com");
    }
}
