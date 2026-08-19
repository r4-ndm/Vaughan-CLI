//! Multi-RPC Quorum Validation for Degen Mode.
//!
//! Validates state across primary + fallback RPC endpoints concurrently
//! to defeat rogue/compromised RPCs or stale data before executing autonomous trades.

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::sol;
use alloy::sol_types::SolCall;
use url::Url;

use crate::error::AgentError;

sol! {
    interface IUniswapV2PairQuorum {
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
    }
}

/// Pair reserves state returned by an RPC query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuorumReserves {
    pub reserve0: U256,
    pub reserve1: U256,
}

/// Quorum validator across multiple RPC URLs.
pub struct QuorumValidator;

impl QuorumValidator {
    /// Concurrently queries `pair` reserves across all provided `rpc_urls`.
    ///
    /// Requires at least `min_quorum` responding RPCs agreeing within 0.1% tolerance.
    pub async fn validate_pair_reserves(
        rpc_urls: &[String],
        pair: Address,
        min_quorum: usize,
    ) -> Result<QuorumReserves, AgentError> {
        if rpc_urls.len() < min_quorum {
            return Err(AgentError::SecurityViolation(format!(
                "Insufficient RPC endpoints configured ({}) for required quorum ({min_quorum})",
                rpc_urls.len()
            )));
        }

        let mut tasks = Vec::new();
        for url_str in rpc_urls {
            let url_s = url_str.clone();
            tasks.push(tokio::spawn(async move {
                let parsed = Url::parse(&url_s).ok()?;
                let provider: RootProvider<alloy::network::Ethereum> =
                    RootProvider::new_http(parsed);
                let call = IUniswapV2PairQuorum::getReservesCall {};
                let tx = alloy::rpc::types::eth::TransactionRequest::default()
                    .to(pair)
                    .input(call.abi_encode().into());

                let res = provider.call(tx).await.ok()?;
                let decoded =
                    IUniswapV2PairQuorum::getReservesCall::abi_decode_returns(&res).ok()?;
                Some(QuorumReserves {
                    reserve0: U256::from(decoded.reserve0),
                    reserve1: U256::from(decoded.reserve1),
                })
            }));
        }

        let mut results = Vec::new();
        for task in tasks {
            if let Ok(Some(res)) = task.await {
                results.push(res);
            }
        }

        if results.len() < min_quorum {
            return Err(AgentError::SecurityViolation(format!(
                "Quorum failed: only {} of {} RPCs responded successfully (required {min_quorum})",
                results.len(),
                rpc_urls.len()
            )));
        }

        // Compare consensus
        let baseline = &results[0];
        let mut matching_count = 0;

        for r in &results {
            if is_within_tolerance(baseline.reserve0, r.reserve0, 10) // 10 bps = 0.1%
                && is_within_tolerance(baseline.reserve1, r.reserve1, 10)
            {
                matching_count += 1;
            }
        }

        if matching_count < min_quorum {
            return Err(AgentError::SecurityViolation(format!(
                "Multi-RPC quorum divergence: {matching_count}/{} agreed on pair reserves (required {min_quorum})",
                results.len()
            )));
        }

        Ok(baseline.clone())
    }
}

fn is_within_tolerance(a: U256, b: U256, max_bps: u32) -> bool {
    if a == b {
        return true;
    }
    let diff = if a > b { a - b } else { b - a };
    let max_val = a.max(b);
    if max_val == U256::ZERO {
        return true;
    }
    let diff_bps = (diff * U256::from(10_000)) / max_val;
    diff_bps <= U256::from(max_bps)
}
