//! Vaughan wallet core: chain adapters, core services, security, persistence.
//!
//! Module layering (keep this order):
//! - `chains` — the family-agnostic [`chains::ChainAdapter`] contract + per-family adapters
//! - `core` — wallet state, accounts, transactions, persistence, network (added as it lands)
//! - `security` — HD wallet + vault encryption
//! - `error` — [`error::WalletError`] + retry helper
//! - `logging` — tracing setup

pub mod chains;
pub mod core;
pub mod error;
pub mod logging;
pub mod security;
