//! Live and mock quote derivation for swap policy enforcement.

use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// Pluggable swap quoter for live on-chain queries or deterministic testing.
#[async_trait]
pub trait SwapQuoter: Send + Sync {
    /// Re-derive quote for Uniswap V2 router.
    async fn quote_v2(
        &self,
        rpc_url: &str,
        router: Address,
        amount_in: U256,
        path: &[Address],
    ) -> Result<U256, String>;

    /// Re-derive quote for Uniswap V3 pool state.
    async fn quote_v3(
        &self,
        rpc_url: &str,
        chain_id: u64,
        token_in: Address,
        token_out: Address,
        fee: u32,
        amount_in: U256,
    ) -> Result<(Address, U256), String>;
}

/// Live on-chain swap quoter using `vaughan-core` RPC services.
pub struct LiveSwapQuoter;

#[async_trait]
impl SwapQuoter for LiveSwapQuoter {
    async fn quote_v2(
        &self,
        rpc_url: &str,
        router: Address,
        amount_in: U256,
        path: &[Address],
    ) -> Result<U256, String> {
        let quote = vaughan_core::core::quote_v2_exact_in(rpc_url, router, amount_in, path)
            .await
            .map_err(|e| e.user_message())?;
        Ok(quote.amount_out)
    }

    async fn quote_v3(
        &self,
        rpc_url: &str,
        chain_id: u64,
        token_in: Address,
        token_out: Address,
        fee: u32,
        amount_in: U256,
    ) -> Result<(Address, U256), String> {
        vaughan_core::core::quote_v3_pool_exact_in(
            rpc_url, chain_id, token_in, token_out, fee, amount_in,
        )
        .await
        .map_err(|e| e.user_message())
    }
}

/// In-memory mock quoter for deterministic policy and calldata unit tests.
#[derive(Debug, Default)]
pub struct MockSwapQuoter {
    pub v2_out: Arc<Mutex<Option<U256>>>,
    pub v3_out: Arc<Mutex<Option<(Address, U256)>>>,
}

impl MockSwapQuoter {
    pub fn new() -> Self {
        Self {
            v2_out: Arc::new(Mutex::new(None)),
            v3_out: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_v2(&self, out: U256) {
        *self.v2_out.lock().unwrap() = Some(out);
    }

    pub fn set_v3(&self, pool: Address, out: U256) {
        *self.v3_out.lock().unwrap() = Some((pool, out));
    }
}

#[async_trait]
impl SwapQuoter for MockSwapQuoter {
    async fn quote_v2(
        &self,
        _rpc_url: &str,
        _router: Address,
        _amount_in: U256,
        _path: &[Address],
    ) -> Result<U256, String> {
        self.v2_out
            .lock()
            .unwrap()
            .ok_or_else(|| "mock V2 quote not configured".to_string())
    }

    async fn quote_v3(
        &self,
        _rpc_url: &str,
        _chain_id: u64,
        _token_in: Address,
        _token_out: Address,
        _fee: u32,
        _amount_in: U256,
    ) -> Result<(Address, U256), String> {
        self.v3_out
            .lock()
            .unwrap()
            .ok_or_else(|| "mock V3 quote not configured".to_string())
    }
}
