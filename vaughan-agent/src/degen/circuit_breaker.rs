//! Hard security circuit breakers and multi-RPC quorum validation for Degen Mode.

use alloy::primitives::U256;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

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
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            max_position_pct: 20,
            max_slippage_bps: 100,                                      // 1%
            max_session_gas_wei: U256::from(50_000_000_000_000_000u64), // 0.05 ETH / 50 PLS
            max_consecutive_errors: 3,
            required_rpc_quorum: 2,
        }
    }
}

/// Dynamic tracker for circuit breaker trip conditions.
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    tripped: Arc<AtomicBool>,
    trip_reason: Arc<RwLock<Option<String>>>,
    consecutive_errors: Arc<AtomicU32>,
    cumulative_gas_wei: Arc<RwLock<U256>>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            tripped: Arc::new(AtomicBool::new(false)),
            trip_reason: Arc::new(RwLock::new(None)),
            consecutive_errors: Arc::new(AtomicU32::new(0)),
            cumulative_gas_wei: Arc::new(RwLock::new(U256::ZERO)),
        }
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

        // 1. Slippage check
        if slippage_bps > self.config.max_slippage_bps {
            self.trip(format!(
                "Slippage violation: requested {slippage_bps} bps exceeds max {} bps",
                self.config.max_slippage_bps
            ));
            return Err(AgentError::CircuitBreakerTripped(
                "Trade exceeded maximum allowable slippage (1.0%)".to_string(),
            ));
        }

        // 2. Position size check
        if total_balance > U256::ZERO {
            let max_allowed =
                (total_balance * U256::from(self.config.max_position_pct)) / U256::from(100);
            if trade_amount > max_allowed {
                self.trip(format!(
                    "Position size violation: trade {trade_amount} exceeds {}% of balance ({max_allowed})",
                    self.config.max_position_pct
                ));
                return Err(AgentError::CircuitBreakerTripped(format!(
                    "Trade exceeds maximum position size ({}%)",
                    self.config.max_position_pct
                )));
            }
        }

        Ok(())
    }

    /// Record a successful transaction and gas expenditure.
    pub fn record_success(&self, gas_used_wei: U256) -> Result<(), AgentError> {
        self.consecutive_errors.store(0, Ordering::SeqCst);
        let mut total_gas = self.cumulative_gas_wei.write().unwrap();
        *total_gas += gas_used_wei;

        if *total_gas > self.config.max_session_gas_wei {
            self.trip(format!(
                "Gas ceiling exceeded: spent {} wei > max {} wei",
                *total_gas, self.config.max_session_gas_wei
            ));
            return Err(AgentError::CircuitBreakerTripped(
                "Trading session gas ceiling exceeded".to_string(),
            ));
        }

        Ok(())
    }

    /// Record a failed transaction and check consecutive error tripwire.
    pub fn record_failure(&self, err_msg: &str) {
        let errs = self.consecutive_errors.fetch_add(1, Ordering::SeqCst) + 1;
        if errs >= self.config.max_consecutive_errors {
            self.trip(format!(
                "Consecutive error tripwire reached ({errs}/{}): last error: {err_msg}",
                self.config.max_consecutive_errors
            ));
        }
    }
}
