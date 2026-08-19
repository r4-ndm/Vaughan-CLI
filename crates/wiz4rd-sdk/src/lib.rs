//! High-level DEX operations for wiz4rd-swap: a PulseChain deployment of the
//! PancakeSwap V3 contracts, driven from Rust.
//!
//! This crate provides the PancakeSwap-specific piece the upstream
//! `uniswap-v3-sdk` crate cannot: **pool address derivation** via CREATE2 with
//! the `PancakeV3PoolDeployer` and PancakeSwap's `POOL_INIT_CODE_HASH`
//! (Uniswap's version derives with the factory + a different hash) — plus the
//! Phase 2 SDK layer: config, token registry, pool reader, offline quotes,
//! swap/liquidity tx builders, allowances, and position reading.

pub mod abi;
pub mod allowance;
pub mod config;
pub mod error;
pub mod pool;
pub mod pool_address;
pub mod positions;
pub mod quote;
pub mod tokens;
pub mod tx;

pub use config::Config;
pub use error::{SdkError, SdkResult};
pub use pool::PoolInfo;
pub use pool_address::{compute_pool_address, get_pool_key, PoolKey};
pub use positions::PositionInfo;
pub use quote::{price_impact_pct, quote_exact_in, quote_exact_out, Quote};
