//! Liquidity pair and pool discovery.
//!
//! Provides hybrid factory indexing:
//! 1. Indexed batch querying (`allPairs(uint256)`) for standard V2/PulseX factories.
//! 2. Event log scanning (`PairCreated` / `PoolCreated`) via `eth_getLogs`.

use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::eth::Filter;
use alloy::rpc::types::BlockNumberOrTag;
use alloy::sol;
use alloy::sol_types::SolCall;
use serde::{Deserialize, Serialize};

// Solidity definitions for V2 Factory calls
sol! {
    interface IUniswapV2Factory {
        function allPairsLength() external view returns (uint256);
        function allPairs(uint256 index) external view returns (address);
        function getPair(address tokenA, address tokenB) external view returns (address);
    }
}

// Canonical topic hashes
pub const PAIR_CREATED_TOPIC: B256 = B256::new([
    0x0d, 0x36, 0x48, 0xbd, 0x0f, 0x6b, 0xa8, 0x01, 0x34, 0xa3, 0x3b, 0xa9, 0x27, 0x5a, 0xc5, 0x85,
    0xd9, 0xd3, 0x15, 0xf0, 0xad, 0x83, 0x55, 0xcd, 0xde, 0xfd, 0xe3, 0x1a, 0xfa, 0x28, 0xd0, 0xe9,
]);

pub const POOL_CREATED_TOPIC: B256 = B256::new([
    0x78, 0x3c, 0xca, 0x1c, 0x04, 0x12, 0xdd, 0x0d, 0x69, 0x5e, 0x78, 0x45, 0x68, 0xc9, 0x6d, 0xa2,
    0xe9, 0xc2, 0x2f, 0xf9, 0x89, 0x35, 0x7a, 0x2e, 0x01, 0xd8, 0xcb, 0x9b, 0x01, 0x9e, 0x70, 0x55,
]);

/// Discovered liquidity pair information.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveredPair {
    pub pair_address: Address,
    pub token0: Option<Address>,
    pub token1: Option<Address>,
    pub block_number: Option<u64>,
}

/// Pair and Pool Discovery Engine.
pub struct PairDiscovery;

impl PairDiscovery {
    /// Query total pairs on a V2 / PulseX factory contract.
    pub async fn get_v2_pairs_count<P: Provider>(
        provider: &P,
        factory: Address,
    ) -> Result<u64, String> {
        let call = IUniswapV2Factory::allPairsLengthCall {};
        let tx = alloy::rpc::types::eth::TransactionRequest::default()
            .to(factory)
            .input(call.abi_encode().into());

        let res = provider
            .call(tx)
            .await
            .map_err(|e| format!("Failed to query allPairsLength: {}", e))?;

        if res.len() >= 32 {
            Ok(U256::from_be_slice(&res[..32]).to::<u64>())
        } else {
            Err("Empty or invalid return data for allPairsLength".to_string())
        }
    }

    /// Fetch a slice of pairs by index from a V2 factory (`allPairs(index)`).
    pub async fn fetch_v2_pairs_range<P: Provider>(
        provider: &P,
        factory: Address,
        start_index: u64,
        limit: u64,
    ) -> Vec<DiscoveredPair> {
        let mut pairs = Vec::new();

        for idx in start_index..(start_index + limit) {
            let call = IUniswapV2Factory::allPairsCall {
                index: U256::from(idx),
            };
            let tx = alloy::rpc::types::eth::TransactionRequest::default()
                .to(factory)
                .input(call.abi_encode().into());

            if let Ok(res) = provider.call(tx).await {
                if res.len() >= 32 {
                    let addr = Address::from_slice(&res[12..32]);
                    if !addr.is_zero() {
                        pairs.push(DiscoveredPair {
                            pair_address: addr,
                            token0: None,
                            token1: None,
                            block_number: None,
                        });
                    }
                }
            }
        }

        pairs
    }

    /// Scan recent `PairCreated` events from a factory in a block range.
    pub async fn scan_pair_created_logs<P: Provider>(
        provider: &P,
        factory: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<DiscoveredPair>, String> {
        let filter = Filter::new()
            .address(factory)
            .event_signature(PAIR_CREATED_TOPIC)
            .from_block(BlockNumberOrTag::Number(from_block))
            .to_block(BlockNumberOrTag::Number(to_block));

        let logs = provider
            .get_logs(&filter)
            .await
            .map_err(|e| format!("get_logs failed: {}", e))?;

        let mut pairs = Vec::new();
        for log in logs {
            let topics = log.topics();
            let data = log.data().data.as_ref();

            let token0 = topics
                .get(1)
                .map(|t| Address::from_slice(&t.as_slice()[12..32]));
            let token1 = topics
                .get(2)
                .map(|t| Address::from_slice(&t.as_slice()[12..32]));

            let pair_address = if data.len() >= 32 {
                Address::from_slice(&data[12..32])
            } else {
                continue;
            };

            pairs.push(DiscoveredPair {
                pair_address,
                token0,
                token1,
                block_number: log.block_number,
            });
        }

        Ok(pairs)
    }
}
