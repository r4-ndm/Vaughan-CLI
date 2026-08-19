//! Deterministic circuit breaker and multi-RPC quorum tests for Degen Mode.

use alloy::primitives::U256;
use vaughan_agent::degen::{CircuitBreaker, CircuitBreakerConfig};

#[test]
fn test_circuit_breaker_position_sizing() {
    let breaker = CircuitBreaker::new(CircuitBreakerConfig {
        max_position_pct: 20, // 20%
        ..Default::default()
    });

    let total_balance = U256::from(100_000);

    // 15% trade is allowed
    assert!(breaker
        .validate_trade(U256::from(15_000), total_balance, 50)
        .is_ok());

    // 25% trade is blocked and trips breaker
    let err = breaker
        .validate_trade(U256::from(25_000), total_balance, 50)
        .unwrap_err();
    assert!(err.to_string().contains("exceeds maximum position size"));
    assert!(breaker.is_tripped());

    // Subsequent valid trade is also blocked because breaker is tripped
    assert!(breaker
        .validate_trade(U256::from(1_000), total_balance, 50)
        .is_err());
}

#[test]
fn test_circuit_breaker_slippage_ceiling() {
    let breaker = CircuitBreaker::new(CircuitBreakerConfig {
        max_slippage_bps: 100, // 1%
        ..Default::default()
    });

    let total_balance = U256::from(100_000);

    // 100 bps (1%) is allowed
    assert!(breaker
        .validate_trade(U256::from(10_000), total_balance, 100)
        .is_ok());

    // 150 bps (1.5%) exceeds hard 1.0% limit
    let err = breaker
        .validate_trade(U256::from(10_000), total_balance, 150)
        .unwrap_err();
    assert!(err.to_string().contains("maximum allowable slippage"));
    assert!(breaker.is_tripped());
}

#[test]
fn test_circuit_breaker_gas_and_error_tripwires() {
    let breaker = CircuitBreaker::new(CircuitBreakerConfig {
        max_session_gas_wei: U256::from(1_000),
        max_consecutive_errors: 3,
        ..Default::default()
    });

    // 1. Consecutive errors
    breaker.record_failure("error 1");
    assert!(!breaker.is_tripped());
    breaker.record_failure("error 2");
    assert!(!breaker.is_tripped());
    breaker.record_failure("error 3");
    assert!(breaker.is_tripped());
    assert!(breaker
        .trip_reason()
        .unwrap()
        .contains("Consecutive error tripwire"));

    // 2. Reset and test gas ceiling
    breaker.reset();
    assert!(!breaker.is_tripped());

    assert!(breaker.record_success(U256::from(600)).is_ok());
    assert!(!breaker.is_tripped());

    assert!(breaker.record_success(U256::from(500)).is_err());
    assert!(breaker.is_tripped());
    assert!(breaker
        .trip_reason()
        .unwrap()
        .contains("Gas ceiling exceeded"));
}

#[test]
fn test_circuit_breaker_emergency_stop() {
    let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
    assert!(!breaker.is_tripped());

    breaker.trip("Emergency stop pressed by user (Esc)");
    assert!(breaker.is_tripped());
    assert_eq!(
        breaker.trip_reason().unwrap(),
        "Emergency stop pressed by user (Esc)"
    );
}
