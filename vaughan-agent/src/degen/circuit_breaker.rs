//! Hard security circuit breakers and multi-RPC quorum validation for Degen Mode.

use alloy::primitives::U256;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use crate::degen::policy::EnforcementMode;
use crate::error::AgentError;

/// Configuration defining strict risk boundaries for autonomous trading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Maximum percentage of wallet balance allowed in a single trade (e.g. 20 = 20%).
    pub max_position_pct: u8,
    /// Hard slippage ceiling in basis points (e.g. 100 bps = 1.00%).
    pub max_slippage_bps: u32,
    /// Cumulative gas cost ceiling in wei before tripping the breaker.
    pub max_session_gas_wei: U256,
    /// Consecutive transaction failure limit before tripping the breaker.
    pub max_consecutive_errors: u32,
    /// Minimum matching RPC count required for quorum validation (e.g. 2).
    pub required_rpc_quorum: usize,
    /// How strictly limits are applied ([`EnforcementMode::Disabled`] = testing).
    #[serde(default)]
    pub enforcement: EnforcementMode,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            // Burner wallet is already isolated — allow up to the full balance in one
            // trade so a user “max N” request is not silently shrunk (still cannot
            // exceed native balance). Slippage / gas / Esc remain hard walls.
            max_position_pct: 100,
            max_slippage_bps: 100,                                      // 1%
            max_session_gas_wei: U256::from(50_000_000_000_000_000u64), // 0.05 ETH / 50 PLS
            max_consecutive_errors: 3,
            required_rpc_quorum: 2,
            enforcement: EnforcementMode::Enforced,
        }
    }
}

/// Dynamic tracker for circuit breaker trip conditions.
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    config: Arc<RwLock<CircuitBreakerConfig>>,
    tripped: Arc<AtomicBool>,
    trip_reason: Arc<RwLock<Option<String>>>,
    consecutive_errors: Arc<AtomicU32>,
    cumulative_gas_wei: Arc<RwLock<U256>>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            tripped: Arc::new(AtomicBool::new(false)),
            trip_reason: Arc::new(RwLock::new(None)),
            consecutive_errors: Arc::new(AtomicU32::new(0)),
            cumulative_gas_wei: Arc::new(RwLock::new(U256::ZERO)),
        }
    }

    /// Replace limits mid-session (from `/policy`); does not clear trip state.
    pub fn replace_config(&self, config: CircuitBreakerConfig) {
        *self.config.write().unwrap() = config;
    }

    /// Check if the circuit breaker is currently tripped.
    pub fn is_tripped(&self) -> bool {
        self.tripped.load(Ordering::SeqCst)
    }

    /// Retrieve the trip reason if tripped.
    pub fn trip_reason(&self) -> Option<String> {
        self.trip_reason.read().unwrap().clone()
    }

    /// Trip the breaker manually (e.g. via emergency stop keypress).
    pub fn trip(&self, reason: impl Into<String>) {
        let r = reason.into();
        *self.trip_reason.write().unwrap() = Some(r);
        self.tripped.store(true, Ordering::SeqCst);
    }

    /// Reset the circuit breaker state.
    pub fn reset(&self) {
        *self.trip_reason.write().unwrap() = None;
        self.consecutive_errors.store(0, Ordering::SeqCst);
        *self.cumulative_gas_wei.write().unwrap() = U256::ZERO;
        self.tripped.store(false, Ordering::SeqCst);
    }

    /// Validate a proposed trade against position sizing and slippage rules.
    ///
    /// Oversized or over-slippage requests are **rejected without** permanently
    /// tripping the session (the model can shrink and retry). Permanent trips
    /// remain for emergency stop, gas ceiling, and consecutive failures.
    ///
    /// [`EnforcementMode::Disabled`] skips size/slippage checks (Esc still trips).
    /// [`EnforcementMode::WarnOnly`] allows the trade but logs a warning.
    pub fn validate_trade(
        &self,
        trade_amount: U256,
        total_balance: U256,
        slippage_bps: u32,
    ) -> Result<(), AgentError> {
        if self.is_tripped() {
            let reason = self.trip_reason().unwrap_or_else(|| "Unknown".to_string());
            return Err(AgentError::CircuitBreakerTripped(format!(
                "Trading halted: {reason}"
            )));
        }

        let cfg = self.config.read().unwrap().clone();
        if cfg.enforcement == EnforcementMode::Disabled {
            return Ok(());
        }

        let mut warnings = Vec::new();

        if slippage_bps > cfg.max_slippage_bps {
            let msg = format!(
                "Slippage {slippage_bps} bps exceeds max {} bps — lower slippage_bps and retry \
                 (session still open)",
                cfg.max_slippage_bps
            );
            if cfg.enforcement == EnforcementMode::WarnOnly {
                warnings.push(msg);
            } else {
                return Err(AgentError::InvalidToolCall(msg));
            }
        }

        if total_balance > U256::ZERO {
            let max_allowed = (total_balance * U256::from(cfg.max_position_pct)) / U256::from(100);
            if trade_amount > max_allowed {
                let msg = format!(
                    "Trade amount {trade_amount} wei exceeds max position size {}% of balance \
                     (max allowed {max_allowed} wei / balance {total_balance} wei). \
                     Reduce amount_in to ≤ {max_allowed} and retry (session still open)",
                    cfg.max_position_pct
                );
                if cfg.enforcement == EnforcementMode::WarnOnly {
                    warnings.push(msg);
                } else {
                    return Err(AgentError::InvalidToolCall(msg));
                }
            }
        }

        for w in warnings {
            tracing::warn!(target: "vaughan_agent::degen", "breaker warn-only: {w}");
        }

        Ok(())
    }

    /// Snapshot of the current config (for prompts / `/policy`).
    pub fn config(&self) -> CircuitBreakerConfig {
        self.config.read().unwrap().clone()
    }

    /// Record a successful transaction and gas expenditure.
    pub fn record_success(&self, gas_used_wei: U256) -> Result<(), AgentError> {
        self.consecutive_errors.store(0, Ordering::SeqCst);
        let cfg = self.config.read().unwrap().clone();
        if cfg.enforcement == EnforcementMode::Disabled {
            return Ok(());
        }

        let mut total_gas = self.cumulative_gas_wei.write().unwrap();
        *total_gas += gas_used_wei;

        if *total_gas > cfg.max_session_gas_wei {
            let msg = format!(
                "Gas ceiling exceeded: spent {} wei > max {} wei",
                *total_gas, cfg.max_session_gas_wei
            );
            if cfg.enforcement == EnforcementMode::WarnOnly {
                tracing::warn!(target: "vaughan_agent::degen", "breaker warn-only: {msg}");
                return Ok(());
            }
            self.trip(msg);
            return Err(AgentError::CircuitBreakerTripped(
                "Trading session gas ceiling exceeded".to_string(),
            ));
        }

        Ok(())
    }

    /// Record a failed transaction and check consecutive error tripwire.
    pub fn record_failure(&self, err_msg: &str) {
        let cfg = self.config.read().unwrap().clone();
        if cfg.enforcement == EnforcementMode::Disabled {
            return;
        }
        let errs = self.consecutive_errors.fetch_add(1, Ordering::SeqCst) + 1;
        if errs >= cfg.max_consecutive_errors {
            let msg = format!(
                "Consecutive error tripwire reached ({errs}/{}): last error: {err_msg}",
                cfg.max_consecutive_errors
            );
            if cfg.enforcement == EnforcementMode::WarnOnly {
                tracing::warn!(target: "vaughan_agent::degen", "breaker warn-only: {msg}");
                return;
            }
            self.trip(msg);
        }
    }
}
