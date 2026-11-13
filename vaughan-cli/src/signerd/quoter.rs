//! Live and mock quote derivation for swap policy enforcement.

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use vaughan_core::core::proposal::DexSwapKind;

sol! {
    interface IPoolV3Probe {
        function fee() external view returns (uint24);
        function token0() external view returns (address);
        function token1() external view returns (address);
    }

    interface IPairV2Probe {
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
        function token0() external view returns (address);
        function token1() external view returns (address);
    }
}

/// Quote output for a DCA slice, including pool kind and route parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DcaQuote {
    pub kind: DexSwapKind,
    pub expected_out: U256,
    pub router: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub fee: Option<u32>,
    pub path: Vec<Address>,
}

impl DcaQuote {
    pub fn unknown() -> Self {
        Self {
            kind: DexSwapKind::Unknown,
            expected_out: U256::ZERO,
            router: Address::ZERO,
            token_in: Address::ZERO,
            token_out: Address::ZERO,
            fee: None,
            path: Vec::new(),
        }
    }
}

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

    /// Re-derive quote for a DCA slice from live pool state.
    async fn quote_dca(
        &self,
        rpc_url: &str,
        chain_id: u64,
        pair: Address,
        amount_in: U256,
    ) -> Result<DcaQuote, String>;
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

    async fn quote_dca(
        &self,
        rpc_url: &str,
        chain_id: u64,
        pair: Address,
        amount_in: U256,
    ) -> Result<DcaQuote, String> {
        if pair == Address::ZERO {
            return Ok(DcaQuote::unknown());
        }

        let url: alloy::transports::http::reqwest::Url =
            rpc_url.parse().map_err(|e| format!("{e}"))?;
        let provider = ProviderBuilder::new().connect_http(url);

        let code = match provider.get_code_at(pair).await {
            Ok(c) => c,
            Err(e) => return Err(format!("get_code_at({pair:#x}): {e}")),
        };
        if code.is_empty() {
            return Ok(DcaQuote::unknown());
        }

        // 1. Probe for Uniswap V3 Pool (fee + token0 + token1)
        let fee_req = TransactionRequest::default()
            .to(pair)
            .input(IPoolV3Probe::feeCall {}.abi_encode().into());
        if let Ok(res_fee) = provider.call(fee_req).await {
            if let Ok(fee_ret) = IPoolV3Probe::feeCall::abi_decode_returns(&res_fee) {
                let fee = fee_ret.to::<u32>();
                let t0_req = TransactionRequest::default()
                    .to(pair)
                    .input(IPoolV3Probe::token0Call {}.abi_encode().into());
                let t1_req = TransactionRequest::default()
                    .to(pair)
                    .input(IPoolV3Probe::token1Call {}.abi_encode().into());
                if let (Ok(res_t0), Ok(res_t1)) =
                    (provider.call(t0_req).await, provider.call(t1_req).await)
                {
                    if let (Ok(t0_ret), Ok(t1_ret)) = (
                        IPoolV3Probe::token0Call::abi_decode_returns(&res_t0),
                        IPoolV3Probe::token1Call::abi_decode_returns(&res_t1),
                    ) {
                        let token0 = Address::from(t0_ret.0);
                        let token1 = Address::from(t1_ret.0);
                        let weth = crate::signerd::intents::resolve_wrapped_native(chain_id)
                            .unwrap_or(token0);
                        let (token_in, token_out) = if token0 == weth {
                            (token0, token1)
                        } else if token1 == weth {
                            (token1, token0)
                        } else {
                            (token0, token1)
                        };
                        let router = vaughan_core::core::v3_swap_routers(chain_id)
                            .next()
                            .ok_or_else(|| format!("no V3 router on chain {chain_id}"))?;
                        // DexSwapKind rule: V3 pool is quoted via quote_v3, never quote_v2
                        let (_pool, expected_out) = self
                            .quote_v3(rpc_url, chain_id, token_in, token_out, fee, amount_in)
                            .await?;
                        return Ok(DcaQuote {
                            kind: DexSwapKind::V3 {
                                pool: pair,
                                fee,
                                quote: expected_out,
                            },
                            expected_out,
                            router,
                            token_in,
                            token_out,
                            fee: Some(fee),
                            path: vec![token_in, token_out],
                        });
                    }
                }
            }
        }

        // 2. Probe for Uniswap V2 Pair (getReserves + token0 + token1)
        let res_req = TransactionRequest::default()
            .to(pair)
            .input(IPairV2Probe::getReservesCall {}.abi_encode().into());
        if let Ok(res_reserves) = provider.call(res_req).await {
            if IPairV2Probe::getReservesCall::abi_decode_returns(&res_reserves).is_ok() {
                let t0_req = TransactionRequest::default()
                    .to(pair)
                    .input(IPairV2Probe::token0Call {}.abi_encode().into());
                let t1_req = TransactionRequest::default()
                    .to(pair)
                    .input(IPairV2Probe::token1Call {}.abi_encode().into());
                if let (Ok(res_t0), Ok(res_t1)) =
                    (provider.call(t0_req).await, provider.call(t1_req).await)
                {
                    if let (Ok(t0_ret), Ok(t1_ret)) = (
                        IPairV2Probe::token0Call::abi_decode_returns(&res_t0),
                        IPairV2Probe::token1Call::abi_decode_returns(&res_t1),
                    ) {
                        let token0 = Address::from(t0_ret.0);
                        let token1 = Address::from(t1_ret.0);
                        let weth = crate::signerd::intents::resolve_wrapped_native(chain_id)
                            .unwrap_or(token0);
                        let path = if token0 == weth {
                            vec![token0, token1]
                        } else if token1 == weth {
                            vec![token1, token0]
                        } else {
                            vec![token0, token1]
                        };
                        let router = vaughan_core::core::v2_swap_routers(chain_id)
                            .next()
                            .ok_or_else(|| format!("no V2 router on chain {chain_id}"))?;
                        let expected_out = self.quote_v2(rpc_url, router, amount_in, &path).await?;
                        return Ok(DcaQuote {
                            kind: DexSwapKind::V2,
                            expected_out,
                            router,
                            token_in: path[0],
                            token_out: path[1],
                            fee: None,
                            path,
                        });
                    }
                }
            }
        }

        // 3. Unknown fails closed
        Ok(DcaQuote::unknown())
    }
}

/// In-memory mock quoter for deterministic policy and calldata unit tests.
#[derive(Debug, Default)]
pub struct MockSwapQuoter {
    pub v2_out: Arc<Mutex<Option<U256>>>,
    pub v3_out: Arc<Mutex<Option<(Address, U256)>>>,
    pub force_kind: Arc<Mutex<Option<DexSwapKind>>>,
    pub dca_tokens: Arc<Mutex<Option<(Address, Address)>>>,
    pub dca_router: Arc<Mutex<Option<Address>>>,
    pub dca_fee: Arc<Mutex<Option<u32>>>,
}

impl MockSwapQuoter {
    pub fn new() -> Self {
        Self {
            v2_out: Arc::new(Mutex::new(None)),
            v3_out: Arc::new(Mutex::new(None)),
            force_kind: Arc::new(Mutex::new(None)),
            dca_tokens: Arc::new(Mutex::new(None)),
            dca_router: Arc::new(Mutex::new(None)),
            dca_fee: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_v2(&self, out: U256) {
        *self.v2_out.lock().unwrap() = Some(out);
    }

    pub fn set_v3(&self, pool: Address, out: U256) {
        *self.v3_out.lock().unwrap() = Some((pool, out));
    }

    pub fn set_force_kind(&self, kind: DexSwapKind) {
        *self.force_kind.lock().unwrap() = Some(kind);
    }

    pub fn set_dca_tokens(&self, token_in: Address, token_out: Address) {
        *self.dca_tokens.lock().unwrap() = Some((token_in, token_out));
    }

    pub fn set_dca_router(&self, router: Address) {
        *self.dca_router.lock().unwrap() = Some(router);
    }

    pub fn set_dca_fee(&self, fee: u32) {
        *self.dca_fee.lock().unwrap() = Some(fee);
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
        let forced = self.force_kind.lock().unwrap().clone();
        if matches!(forced, Some(DexSwapKind::V3 { .. })) {
            return Err(
                "DexSwapKind violation: V3 pool cannot be routed through V2 quoter".to_string(),
            );
        }
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
        let forced = self.force_kind.lock().unwrap().clone();
        if matches!(forced, Some(DexSwapKind::V2)) {
            return Err(
                "DexSwapKind violation: V2 pool cannot be routed through V3 quoter".to_string(),
            );
        }
        self.v3_out
            .lock()
            .unwrap()
            .ok_or_else(|| "mock V3 quote not configured".to_string())
    }

    async fn quote_dca(
        &self,
        rpc_url: &str,
        chain_id: u64,
        pair: Address,
        amount_in: U256,
    ) -> Result<DcaQuote, String> {
        let forced = self.force_kind.lock().unwrap().clone();
        if let Some(DexSwapKind::Unknown) = forced {
            return Ok(DcaQuote::unknown());
        }

        let is_v3 = matches!(forced, Some(DexSwapKind::V3 { .. }))
            || (forced.is_none() && self.v3_out.lock().unwrap().is_some());
        let is_v2 = matches!(forced, Some(DexSwapKind::V2))
            || (forced.is_none() && self.v2_out.lock().unwrap().is_some());

        if is_v3 {
            let (token_in, token_out) = self
                .dca_tokens
                .lock()
                .unwrap()
                .unwrap_or_else(|| (Address::repeat_byte(0x22), Address::repeat_byte(0x33)));
            let fee = self.dca_fee.lock().unwrap().unwrap_or(3000);
            let router = self.dca_router.lock().unwrap().unwrap_or_else(|| {
                vaughan_core::core::v3_swap_routers(chain_id)
                    .next()
                    .unwrap_or_else(|| Address::repeat_byte(0x77))
            });
            let (_pool, expected_out) = self
                .quote_v3(rpc_url, chain_id, token_in, token_out, fee, amount_in)
                .await?;
            return Ok(DcaQuote {
                kind: DexSwapKind::V3 {
                    pool: pair,
                    fee,
                    quote: expected_out,
                },
                expected_out,
                router,
                token_in,
                token_out,
                fee: Some(fee),
                path: vec![token_in, token_out],
            });
        }

        if is_v2 {
            let (token_in, token_out) = self
                .dca_tokens
                .lock()
                .unwrap()
                .unwrap_or_else(|| (Address::repeat_byte(0x22), Address::repeat_byte(0x33)));
            let router = self.dca_router.lock().unwrap().unwrap_or_else(|| {
                vaughan_core::core::v2_swap_routers(chain_id)
                    .next()
                    .unwrap_or_else(|| Address::repeat_byte(0x88))
            });
            let path = vec![token_in, token_out];
            let expected_out = self.quote_v2(rpc_url, router, amount_in, &path).await?;
            return Ok(DcaQuote {
                kind: DexSwapKind::V2,
                expected_out,
                router,
                token_in,
                token_out,
                fee: None,
                path,
            });
        }

        Ok(DcaQuote::unknown())
    }
}
