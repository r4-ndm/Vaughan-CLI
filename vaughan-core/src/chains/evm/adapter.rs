//! Alloy-backed EVM chain adapter.
//!
//! Implements [`ChainAdapter`] for native EVM asset operations: balance,
//! fee estimation, and transaction signing/broadcast. EVM-specific operations
//! (nonce, ERC-20) live on [`EvmAdapter`] itself rather than the shared trait.

use std::collections::HashSet;
use std::future::Future;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use alloy::eips::eip2718::Encodable2718;
use alloy::network::{Ethereum, EthereumWallet, NetworkTransactionBuilder};
use alloy::primitives::{keccak256, utils::format_units, Address, Bytes, TxKind, B256, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::client::RpcClient;
use alloy::rpc::types::eth::{Filter, TransactionRequest};
use alloy::rpc::types::BlockNumberOrTag;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolCall;
use alloy::transports::http::Http;
use async_trait::async_trait;
use url::Url;

use crate::chains::evm::abi::{IERC20Metadata, IMulticall3};
use crate::chains::evm::networks::get_network_by_chain_id;
use crate::chains::evm::tokens::tokens_for_chain;
use crate::chains::evm::utils::parse_address;
use crate::chains::{
    Balance, ChainAdapter, ChainInfo, ChainTransaction, ChainType, EvmTransaction, Fee, FeeDetails,
    TokenInfo, TxHash, TxRecord, TxStatus,
};
use crate::error::WalletError;

pub type AlloyProvider = RootProvider<Ethereum>;

/// Default EIP-1559 priority fee (tip) when the network config doesn't specify
/// one. Networks with a different market (e.g. PulseChain's sub-gwei fees)
/// override this in [`crate::chains::evm::networks`].
pub const DEFAULT_PRIORITY_FEE_WEI: u64 = 1_500_000_000; // 1.5 gwei

/// Canonical Multicall3 address (https://github.com/mds1/multicall). Present
/// on both PulseChain mainnet and testnet — verified via `cast codesize`
/// (3808 bytes on both, 2026-08-18; see `docs/optimizations.md`).
pub const MULTICALL3: Address =
    alloy::primitives::address!("cA11bde05977b3631167028862bE2a173976CA11");

/// EIP-20 `Transfer` event topic0 — `keccak256("Transfer(address,address,uint256)")`
/// (EIP-20 §1). Auto asset discovery scans logs with this signature to find
/// every token the wallet has ever sent or received. Computed once (alloy's
/// `keccak256` is not const).
pub fn transfer_topic0() -> B256 {
    keccak256(b"Transfer(address,address,uint256)")
}

/// How far back auto asset discovery scans `Transfer` logs (blocks).
/// Bounded so the `getLogs` call stays cheap on every scan; tokens received
/// before the window are still reachable via the curated list or a raw
/// address query.
pub const DISCOVERY_BLOCK_WINDOW: u64 = 1_000_000;

/// EVM adapter built on Alloy's HTTP provider, with transparent fallback to
/// alternate RPC endpoints when the primary is down or rate-limited.
pub struct EvmAdapter {
    /// Primary (index 0) plus fallback providers, tried in order.
    providers: Vec<Arc<AlloyProvider>>,
    /// Index of the provider that last answered; used as the starting point
    /// for the next call so a dead primary doesn't stall every request.
    active_provider: AtomicUsize,
    signer: Option<PrivateKeySigner>,
    rpc_url: String,
    chain_id: u64,
    network_name: String,
    /// Network-specific EIP-1559 tip used by `estimate_fee`.
    priority_fee_wei: u64,
    balance_cache: moka::future::Cache<String, Balance>,
    nonce_cache: moka::future::Cache<String, u64>,
    /// ERC-20 metadata cache: token address → (symbol, name, decimals).
    token_meta_cache: moka::future::Cache<String, (String, String, u8)>,
}

impl EvmAdapter {
    /// Create an EVM adapter for the given RPC URL and chain id.
    ///
    /// `fallback_rpc_urls` are tried in order when the primary fails; invalid
    /// URLs are skipped with a warning rather than failing construction.
    pub async fn new(
        rpc_url: &str,
        chain_id: u64,
        network_name: impl Into<String>,
        fallback_rpc_urls: &[String],
    ) -> Result<Self, WalletError> {
        let mut providers = Vec::with_capacity(fallback_rpc_urls.len() + 1);
        providers.push(Arc::new(Self::build_provider(rpc_url)?));
        for url in fallback_rpc_urls {
            match Self::build_provider(url) {
                Ok(provider) => providers.push(Arc::new(provider)),
                Err(e) => tracing::warn!("skipping invalid fallback RPC {url}: {e}"),
            }
        }
        // Network-specific default tip (audit 4.2); generic 1.5 gwei otherwise.
        let priority_fee_wei = get_network_by_chain_id(chain_id)
            .and_then(|net| net.default_priority_fee_wei)
            .unwrap_or(DEFAULT_PRIORITY_FEE_WEI);
        Ok(Self {
            providers,
            active_provider: AtomicUsize::new(0),
            signer: None,
            rpc_url: rpc_url.to_string(),
            chain_id,
            network_name: network_name.into(),
            priority_fee_wei,
            balance_cache: moka::future::Cache::builder()
                .time_to_live(Duration::from_secs(10))
                .build(),
            nonce_cache: moka::future::Cache::builder()
                .time_to_live(Duration::from_secs(5))
                .build(),
            token_meta_cache: moka::future::Cache::builder()
                .time_to_live(Duration::from_secs(60 * 60))
                .build(),
        })
    }

    /// Create an adapter with a local signer for signing/sending.
    pub async fn with_signer(
        rpc_url: &str,
        chain_id: u64,
        network_name: impl Into<String>,
        signer: PrivateKeySigner,
        fallback_rpc_urls: &[String],
    ) -> Result<Self, WalletError> {
        let mut this = Self::new(rpc_url, chain_id, network_name, fallback_rpc_urls).await?;
        this.signer = Some(signer);
        Ok(this)
    }

    /// Set (or replace) the local signer.
    pub fn set_signer(&mut self, signer: PrivateKeySigner) {
        self.signer = Some(signer);
    }

    /// The primary provider (first configured endpoint).
    pub fn provider(&self) -> Arc<AlloyProvider> {
        self.providers[0].clone()
    }

    /// Build an HTTP-backed provider for `rpc_url`.
    fn build_provider(rpc_url: &str) -> Result<AlloyProvider, WalletError> {
        let url = Url::parse(rpc_url).map_err(|e| WalletError::NetworkError(e.to_string()))?;
        let transport = Http::new(url);
        let client = RpcClient::new(transport, true);
        Ok(RootProvider::<Ethereum>::new(client))
    }

    /// Run `call` against the provider chain: the last-known-good endpoint
    /// first, then the others in order. Only transport-ish failures (RPC,
    /// network, gas estimation, broadcast) trigger a fallback; validation or
    /// signing errors fail fast.
    pub async fn with_provider<T, F, Fut>(&self, call: F) -> Result<T, WalletError>
    where
        F: Fn(Arc<AlloyProvider>) -> Fut,
        Fut: Future<Output = Result<T, WalletError>>,
    {
        let start = self.active_provider.load(Ordering::Relaxed);
        let mut last_err: Option<WalletError> = None;
        for offset in 0..self.providers.len() {
            let index = (start + offset) % self.providers.len();
            match call(self.providers[index].clone()).await {
                Ok(value) => {
                    if index != start {
                        self.active_provider.store(index, Ordering::Relaxed);
                    }
                    return Ok(value);
                }
                Err(e) if Self::is_transport_failure(&e) => last_err = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last_err
            .unwrap_or_else(|| WalletError::RpcError("all RPC endpoints failed".to_string())))
    }

    /// True for errors that are worth retrying against another endpoint.
    fn is_transport_failure(e: &WalletError) -> bool {
        matches!(
            e,
            WalletError::RpcError(_)
                | WalletError::NetworkError(_)
                | WalletError::GasEstimationFailed(_)
                | WalletError::TransactionFailed(_)
        )
    }

    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Suggested max fee / legacy gas price formatted as gwei for wallet status chrome.
    pub async fn gas_price_gwei_display(&self) -> Result<String, WalletError> {
        let wei = self
            .with_provider(|provider| async move {
                match provider.estimate_eip1559_fees().await {
                    Ok(est) => Ok(U256::from(est.max_fee_per_gas)),
                    Err(_) => {
                        let gas_price = provider
                            .get_gas_price()
                            .await
                            .map_err(|e| WalletError::RpcError(e.to_string()))?;
                        Ok(U256::from(gas_price))
                    }
                }
            })
            .await?;
        // 1 gwei = 1e9 wei; show up to 4 decimal places when sub-gwei.
        let gwei = format_units(wei, 9).unwrap_or_else(|_| "0".into());
        let trimmed = gwei.trim_end_matches('0').trim_end_matches('.');
        Ok(if trimmed.is_empty() {
            "0".into()
        } else {
            trimmed.to_string()
        })
    }

    /// Native asset metadata for the configured chain (symbol, name, decimals).
    fn native_asset(&self) -> (String, String, u8) {
        if let Some(net) = get_network_by_chain_id(self.chain_id) {
            (net.native_symbol, net.native_name, net.decimals)
        } else {
            ("ETH".to_string(), "Ethereum".to_string(), 18)
        }
    }

    /// EVM-specific: fetch the next transaction nonce for `address`.
    ///
    /// **Read/display paths only.** The value is cached for 5s, so it can go
    /// stale between sends; transaction submission must query the pending
    /// nonce directly (see [`Self::get_pending_nonce`]) or reuse a
    /// previously-returned nonce.
    pub async fn get_nonce(&self, address: &str) -> Result<u64, WalletError> {
        if let Some(cached) = self.nonce_cache.get(address).await {
            return Ok(cached);
        }
        let addr = parse_address(address)?;
        let nonce = self
            .with_provider(|provider| async move {
                provider
                    .get_transaction_count(addr)
                    .await
                    .map_err(|e| WalletError::RpcError(e.to_string()))
            })
            .await?;
        self.nonce_cache.insert(address.to_string(), nonce).await;
        Ok(nonce)
    }

    /// EVM-specific: query the *pending* transaction count for `address` —
    /// the nonce the next submitted transaction must use.
    ///
    /// Unlike [`Self::get_nonce`], this is **never cached**, so it is safe for
    /// submission paths where a 5s-TTL cache could reuse a nonce across rapid
    /// successive sends. Same `with_provider` fallback semantics as the rest
    /// of the adapter.
    pub async fn get_pending_nonce(&self, address: &str) -> Result<u64, WalletError> {
        let addr = parse_address(address)?;
        self.with_provider(|provider| async move {
            provider
                .get_transaction_count(addr)
                .pending()
                .await
                .map_err(|e| WalletError::RpcError(e.to_string()))
        })
        .await
    }

    /// Broadcast an already-signed raw transaction (EIP-2718 encoded, no
    /// leading `0x`) through the primary + fallback provider chain.
    ///
    /// Broadcasting the same signed envelope to a fallback endpoint is safe:
    /// identical nonce means a duplicate is a no-op on-chain.
    pub async fn broadcast_raw(&self, raw: Vec<u8>) -> Result<TxHash, WalletError> {
        let pending = self
            .with_provider(move |provider| {
                let raw = raw.clone();
                async move {
                    provider
                        .send_raw_transaction(&raw)
                        .await
                        .map_err(|e| WalletError::TransactionFailed(e.to_string()))
                }
            })
            .await?;
        Ok(TxHash(format!("{:?}", pending.tx_hash())))
    }

    /// EVM-specific: ERC-20 `balanceOf` (EIP-20) for `wallet_address`, with
    /// symbol/decimals resolved on-chain (cached).
    pub async fn get_token_balance(
        &self,
        token_address: &str,
        wallet_address: &str,
    ) -> Result<Balance, WalletError> {
        let cache_key = format!("{token_address}:{wallet_address}").to_ascii_lowercase();
        if let Some(cached) = self.balance_cache.get(&cache_key).await {
            return Ok(cached);
        }
        let token = parse_address(token_address)?;
        let wallet = parse_address(wallet_address)?;
        let input = IERC20Metadata::balanceOfCall { account: wallet }.abi_encode();
        let raw = self
            .with_provider(|provider| {
                let input = input.clone();
                async move {
                    provider
                        .call(TransactionRequest::default().to(token).input(input.into()))
                        .await
                        .map_err(|e| WalletError::RpcError(e.to_string()))
                }
            })
            .await?;
        // An address with no code (token not deployed on this chain) returns
        // `0x` as a *successful* call — treat that as a zero balance rather
        // than failing the whole read.
        let raw_balance = if raw.is_empty() {
            U256::ZERO
        } else {
            IERC20Metadata::balanceOfCall::abi_decode_returns(&raw)
                .map_err(|e| WalletError::Other(format!("bad balanceOf return: {e}")))?
        };
        let (symbol, name, decimals) = self.get_token_metadata(token_address).await?;
        let formatted = format_units(raw_balance, decimals).unwrap_or_else(|_| "0.0".to_string());
        let bal = Balance {
            token: TokenInfo {
                symbol,
                name,
                decimals,
                contract_address: Some(token_address.to_string()),
            },
            raw: raw_balance.to_string(),
            formatted,
            usd_value: None,
        };
        self.balance_cache.insert(cache_key, bal.clone()).await;
        Ok(bal)
    }

    /// EVM-specific: best-effort ERC-20 metadata (EIP-20 `symbol`/`name`/
    /// `decimals`), cached for an hour. Each accessor falls back individually
    /// (EIP-20 does not require any of them) — the registry entry wins when
    /// the contract doesn't provide the value, and a raw address defaults to
    /// a shortened address + 18 decimals.
    pub async fn get_token_metadata(
        &self,
        token_address: &str,
    ) -> Result<(String, String, u8), WalletError> {
        let key = token_address.to_ascii_lowercase();
        if let Some(cached) = self.token_meta_cache.get(&key).await {
            return Ok(cached);
        }
        let token = parse_address(token_address)?;
        let registry = crate::chains::evm::find_token(self.chain_id, token_address);

        // `symbol()` — fallback: registry symbol, else a shortened address.
        let symbol = self
            .eth_call_symbol(token)
            .await
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                registry
                    .as_ref()
                    .map(|t| t.symbol.to_string())
                    .unwrap_or_else(|| short_address(token))
            });
        // `name()` — fallback: symbol, else registry name.
        let name = self
            .eth_call_name(token)
            .await
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                registry
                    .as_ref()
                    .map(|t| t.name.to_string())
                    .unwrap_or_else(|| symbol.clone())
            });
        // `decimals()` — fallback: registry decimals, else 18.
        let decimals = self
            .eth_call_decimals(token)
            .await
            .unwrap_or_else(|| registry.as_ref().map(|t| t.decimals).unwrap_or(18));
        let meta = (symbol, name, decimals);
        self.token_meta_cache.insert(key, meta.clone()).await;
        Ok(meta)
    }

    /// EVM-specific: all balances of `wallet_address` — the native asset plus
    /// every ERC-20 the wallet holds, detected from:
    ///
    /// 1. the **curated per-chain registry** (see `tokens.rs`);
    /// 2. **`Transfer`-event discovery**;
    /// 3. **`extra_token_addresses`** — user-imported custom tokens (always
    ///    listed, including zero balance).
    ///
    /// ERC-20 balances are read in **one** Multicall3 `tryAggregate` call
    /// (provenance: mds1/multicall) with `requireSuccess=false`, so a token
    /// that reverts on `balanceOf` is skipped rather than failing the batch.
    /// When Multicall3 is absent on the chain the call falls back to
    /// sequential `balanceOf` reads.
    pub async fn get_assets(
        &self,
        wallet_address: &str,
        extra_token_addresses: &[String],
    ) -> Result<Vec<Balance>, WalletError> {
        let wallet = parse_address(wallet_address)?;
        let mut out = vec![self.get_balance(wallet_address).await?];

        // Token set: curated registry + Transfer-event discovery + imports.
        let mut addresses: Vec<String> = tokens_for_chain(self.chain_id)
            .into_iter()
            .map(|t| t.address.to_string())
            .collect();
        for addr in self.discover_token_addresses(wallet_address).await? {
            let a = addr.to_string();
            if !addresses
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&a))
            {
                addresses.push(a);
            }
        }
        let mut always_include: Vec<String> = Vec::new();
        for raw in extra_token_addresses {
            let a = raw.trim().to_string();
            if a.is_empty() {
                continue;
            }
            if !addresses
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&a))
            {
                addresses.push(a.clone());
            }
            if !always_include
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&a))
            {
                always_include.push(a);
            }
        }

        if addresses.is_empty() {
            return Ok(out);
        }

        // Only use the Multicall3 batch if the contract is actually deployed
        // on this chain: an `eth_call` to an address with no code returns
        // `0x` as *success*, so without this probe the decode would fail
        // instead of falling back to sequential reads.
        let multicall3_present = self
            .with_provider(|provider| async move {
                provider
                    .get_code_at(MULTICALL3)
                    .await
                    .map(|code| !code.is_empty())
                    .map_err(|e| WalletError::RpcError(e.to_string()))
            })
            .await
            .unwrap_or(false);

        if !multicall3_present {
            return self
                .get_assets_sequential(wallet_address, addresses, &always_include, out)
                .await;
        }

        let calls: Vec<IMulticall3::Call> = addresses
            .iter()
            .map(|address| IMulticall3::Call {
                target: parse_address(address).expect("addresses are valid"),
                callData: IERC20Metadata::balanceOfCall { account: wallet }
                    .abi_encode()
                    .into(),
            })
            .collect();
        let req = TransactionRequest::default().to(MULTICALL3).input(
            IMulticall3::tryAggregateCall {
                requireSuccess: false,
                calls,
            }
            .abi_encode()
            .into(),
        );

        let multicall = self
            .with_provider(|provider| {
                let req = req.clone();
                async move {
                    provider
                        .call(req)
                        .await
                        .map_err(|e| WalletError::RpcError(e.to_string()))
                }
            })
            .await;

        match multicall {
            Ok(raw) => {
                let ret = IMulticall3::tryAggregateCall::abi_decode_returns(&raw)
                    .map_err(|e| WalletError::Other(format!("bad tryAggregate return: {e}")))?;
                // `Result[]` — one `(success, returnData)` struct per call.
                for (address, result) in addresses.iter().zip(ret.iter()) {
                    if !result.success {
                        continue; // reverting token — skip, don't fail the batch
                    }
                    match IERC20Metadata::balanceOfCall::abi_decode_returns(&result.returnData) {
                        Ok(raw_balance) => {
                            let keep = !raw_balance.is_zero()
                                || always_include
                                    .iter()
                                    .any(|a| a.eq_ignore_ascii_case(address));
                            if !keep {
                                continue;
                            }
                            let (symbol, name, decimals) = self.get_token_metadata(address).await?;
                            out.push(Balance {
                                token: TokenInfo {
                                    symbol,
                                    name,
                                    decimals,
                                    contract_address: Some(address.clone()),
                                },
                                raw: raw_balance.to_string(),
                                formatted: format_units(raw_balance, decimals)
                                    .unwrap_or_else(|_| "0.0".to_string()),
                                usd_value: None,
                            });
                        }
                        _ => continue,
                    }
                }
            }
            Err(e) => {
                return Err(WalletError::RpcError(format!(
                    "Multicall3 batch failed: {e}"
                )))
            }
        }
        Ok(out)
    }

    /// Sequential per-token fallback when Multicall3 is absent (or the batch
    /// path can't run). One `eth_call` per token — same result set as the
    /// batch, just more RPC round-trips.
    async fn get_assets_sequential(
        &self,
        wallet_address: &str,
        addresses: Vec<String>,
        always_include: &[String],
        mut out: Vec<Balance>,
    ) -> Result<Vec<Balance>, WalletError> {
        for address in &addresses {
            let bal = self.get_token_balance(address, wallet_address).await?;
            let raw = U256::from_str(&bal.raw).unwrap_or_default();
            let keep = raw != U256::ZERO
                || always_include
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(address));
            if keep {
                out.push(bal);
            }
        }
        Ok(out)
    }

    /// Auto asset discovery: scan EIP-20 `Transfer` logs (topic0
    /// [`transfer_topic0`]) involving `wallet_address` over the last
    /// [`DISCOVERY_BLOCK_WINDOW`] blocks and return the unique token
    /// addresses seen. Tokens sent *or* received count (both directions), so
    /// anything the wallet has ever interacted with shows up in Assets even
    /// when it's not on the curated list.
    ///
    /// The window is bounded so `getLogs` stays cheap per scan; this is a
    /// best-effort helper — RPC `getLogs` limits (block range, result size)
    /// may silently truncate, so the curated list remains the trusted seed.
    pub async fn discover_token_addresses(
        &self,
        wallet_address: &str,
    ) -> Result<Vec<Address>, WalletError> {
        let wallet = parse_address(wallet_address)?;
        let latest = self
            .with_provider(|provider| async move {
                provider
                    .get_block_number()
                    .await
                    .map_err(|e| WalletError::RpcError(e.to_string()))
            })
            .await?;
        let from = latest.saturating_sub(DISCOVERY_BLOCK_WINDOW);

        // Two queries (OR is not expressible across topic positions):
        // 1. wallet as `to` (received), 2. wallet as `from` (sent).
        let mut seen = HashSet::new();
        for filter in [
            Filter::new()
                .from_block(from)
                .to_block(latest)
                .event_signature(transfer_topic0())
                .topic2(wallet),
            Filter::new()
                .from_block(from)
                .to_block(latest)
                .event_signature(transfer_topic0())
                .topic1(wallet),
        ] {
            let logs = self
                .with_provider(|provider| {
                    let filter = filter.clone();
                    async move {
                        provider
                            .get_logs(&filter)
                            .await
                            .map_err(|e| WalletError::RpcError(e.to_string()))
                    }
                })
                .await?;
            for log in logs {
                seen.insert(log.inner.address);
            }
        }
        Ok(seen.into_iter().collect())
    }

    /// Best-effort `symbol()` read for `token` (EIP-20 metadata). A revert
    /// (token without `symbol`) yields `None`.
    async fn eth_call_symbol(&self, token: Address) -> Option<String> {
        let call = IERC20Metadata::symbolCall {};
        let raw = self.eth_call_raw(token, &call).await?;
        IERC20Metadata::symbolCall::abi_decode_returns(&raw).ok()
    }

    /// Best-effort `name()` read for `token` (EIP-20 metadata).
    async fn eth_call_name(&self, token: Address) -> Option<String> {
        let call = IERC20Metadata::nameCall {};
        let raw = self.eth_call_raw(token, &call).await?;
        IERC20Metadata::nameCall::abi_decode_returns(&raw).ok()
    }

    /// Best-effort `decimals()` read for `token` (EIP-20 metadata).
    async fn eth_call_decimals(&self, token: Address) -> Option<u8> {
        let call = IERC20Metadata::decimalsCall {};
        let raw = self.eth_call_raw(token, &call).await?;
        IERC20Metadata::decimalsCall::abi_decode_returns(&raw).ok()
    }

    /// Run `call` against `token` via `eth_call` (typed encoding, typed
    /// decoding happens at the call site). `None` on any revert/transport
    /// error — metadata accessors are best-effort by design.
    async fn eth_call_raw<C: SolCall>(&self, token: Address, call: &C) -> Option<Bytes> {
        let input = call.abi_encode();
        self.with_provider(|provider| {
            let input = input.clone();
            async move {
                provider
                    .call(TransactionRequest::default().to(token).input(input.into()))
                    .await
                    .map_err(|e| WalletError::RpcError(e.to_string()))
            }
        })
        .await
        .ok()
    }

    /// EVM-specific: ERC-20 transfer history (not yet implemented).
    pub async fn get_token_transfer_history(
        &self,
        _address: &str,
        _limit: u32,
    ) -> Result<Vec<TxRecord>, WalletError> {
        Ok(Vec::new())
    }

    /// Build and sign an EVM transaction, returning the raw signed envelope
    /// (EIP-2718 encoded, no leading `0x`).
    ///
    /// Auto-fills the nonce from the pending pool when the caller didn't
    /// supply one (updating `evm_tx.nonce` in place) so a missing nonce never
    /// reaches signing. Gas and fee parameters are taken from the transaction
    /// as provided — the caller (wallet core) fills them from a fee estimate
    /// first when they are absent.
    async fn build_signed_envelope(
        &self,
        evm_tx: &mut EvmTransaction,
    ) -> Result<Vec<u8>, WalletError> {
        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| WalletError::SigningFailed("No signer configured".to_string()))?;
        // The *pending* nonce is queried directly (never the cached one): a
        // 5s-TTL cache would reuse a nonce across rapid successive sends and
        // produce "nonce too low" errors.
        if evm_tx.nonce.is_none() {
            let nonce = self.get_pending_nonce(&evm_tx.from).await?;
            evm_tx.nonce = Some(nonce);
        }
        let from = parse_address(&evm_tx.from)?;
        let to = parse_address(&evm_tx.to)?;
        let value = U256::from_str(&evm_tx.value).map_err(|_| {
            WalletError::InvalidAmount(format!("Invalid wei value: {}", evm_tx.value))
        })?;
        // Zero `to` + calldata = contract creation (TxKind::Create). A plain
        // value transfer to the zero address is a burn, not a create.
        let is_create = to.is_zero()
            && evm_tx
                .data
                .as_deref()
                .is_some_and(|d| !d.trim_start_matches("0x").is_empty());
        let mut req = TransactionRequest {
            from: Some(from),
            to: Some(if is_create {
                TxKind::Create
            } else {
                TxKind::Call(to)
            }),
            value: Some(value),
            chain_id: Some(evm_tx.chain_id),
            nonce: evm_tx.nonce,
            gas: evm_tx.gas_limit,
            ..Default::default()
        };
        // Legacy `gasPrice` is only set when no EIP-1559 fees are present:
        // RPCs reject requests carrying both.
        if evm_tx.max_fee_per_gas.is_none() {
            if let Some(gas_price) = evm_tx.gas_price.as_deref() {
                let gp = U256::from_str(gas_price).map_err(|_| {
                    WalletError::InvalidAmount(format!("Invalid gas price: {gas_price}"))
                })?;
                req.gas_price = Some(gp.to::<u128>());
            }
        }
        if let Some(max_fee) = evm_tx.max_fee_per_gas.as_deref() {
            let mf = U256::from_str(max_fee)
                .map_err(|_| WalletError::InvalidAmount(format!("Invalid max fee: {max_fee}")))?;
            req.max_fee_per_gas = Some(mf.to::<u128>());
        }
        if let Some(prio) = evm_tx.max_priority_fee_per_gas.as_deref() {
            let p = U256::from_str(prio)
                .map_err(|_| WalletError::InvalidAmount(format!("Invalid priority fee: {prio}")))?;
            req.max_priority_fee_per_gas = Some(p.to::<u128>());
        }
        if let Some(data_hex) = evm_tx.data.as_deref() {
            let input_bytes = hex::decode(data_hex.trim_start_matches("0x"))
                .map_err(|_| WalletError::InvalidTransaction("Invalid hex data".to_string()))?;
            req.input.input = Some(input_bytes.into());
        }

        let wallet = EthereumWallet::from(signer.clone());
        let envelope = req
            .build(&wallet)
            .await
            .map_err(|e| WalletError::SigningFailed(e.to_string()))?;
        Ok(envelope.encoded_2718())
    }

    /// Sign a transaction without broadcasting it; returns the raw signed tx
    /// as `0x`-prefixed hex. Serves `vaughan_signTransaction` (the Freedom
    /// Browser signer backend, which populates nonce/fees and broadcasts via
    /// its own RPC pool).
    pub async fn sign_transaction(&self, tx: ChainTransaction) -> Result<String, WalletError> {
        let mut evm_tx = match tx {
            ChainTransaction::Evm(evm_tx) => evm_tx,
            _ => {
                return Err(WalletError::InvalidTransaction(
                    "expected an EVM transaction".to_string(),
                ));
            }
        };
        let raw = self.build_signed_envelope(&mut evm_tx).await?;
        Ok(format!("0x{}", hex::encode(raw)))
    }
}

/// Short display form for an unknown token address (`0x1234…abcd`).
fn short_address(a: Address) -> String {
    let s = a.to_string();
    format!("{}…{}", &s[..8], &s[s.len() - 4..])
}

#[async_trait]
impl ChainAdapter for EvmAdapter {
    fn chain_type(&self) -> ChainType {
        ChainType::Evm
    }

    fn chain_info(&self) -> ChainInfo {
        if let Some(net) = get_network_by_chain_id(self.chain_id) {
            ChainInfo {
                chain_type: ChainType::Evm,
                network_id: net.chain_id.to_string(),
                name: net.name,
                rpc_url: net.rpc_url,
            }
        } else {
            ChainInfo {
                chain_type: ChainType::Evm,
                network_id: self.chain_id.to_string(),
                name: self.network_name.clone(),
                rpc_url: self.rpc_url.clone(),
            }
        }
    }

    fn validate_address(&self, address: &str) -> Result<(), WalletError> {
        parse_address(address).map(|_| ())
    }

    async fn get_balance(&self, address: &str) -> Result<Balance, WalletError> {
        if let Some(cached) = self.balance_cache.get(address).await {
            return Ok(cached);
        }
        let addr = parse_address(address)?;
        let raw = self
            .with_provider(|provider| async move {
                provider
                    .get_balance(addr)
                    .await
                    .map_err(|e| WalletError::RpcError(e.to_string()))
            })
            .await?;
        let (symbol, name, decimals) = self.native_asset();
        let formatted = format_units(raw, decimals).unwrap_or_else(|_| "0.0".to_string());
        let bal = Balance {
            token: TokenInfo {
                symbol,
                name,
                decimals,
                contract_address: None,
            },
            raw: raw.to_string(),
            formatted,
            usd_value: None,
        };
        self.balance_cache
            .insert(address.to_string(), bal.clone())
            .await;
        Ok(bal)
    }

    async fn estimate_fee(&self, tx: &ChainTransaction) -> Result<Fee, WalletError> {
        let ChainTransaction::Evm(evm_tx) = tx else {
            return Err(WalletError::InvalidTransaction(
                "expected an EVM transaction".to_string(),
            ));
        };
        let from = parse_address(&evm_tx.from)?;
        let to = parse_address(&evm_tx.to)?;
        let value = U256::from_str(&evm_tx.value).map_err(|_| {
            WalletError::InvalidAmount(format!("Invalid wei value: {}", evm_tx.value))
        })?;
        let is_create = to.is_zero()
            && evm_tx
                .data
                .as_deref()
                .is_some_and(|d| !d.trim_start_matches("0x").is_empty());
        let mut req = TransactionRequest {
            from: Some(from),
            to: Some(if is_create {
                TxKind::Create
            } else {
                TxKind::Call(to)
            }),
            value: Some(value),
            ..Default::default()
        };
        if let Some(data_hex) = evm_tx.data.as_deref() {
            let input_bytes = hex::decode(data_hex.trim_start_matches("0x"))
                .map_err(|_| WalletError::InvalidTransaction("Invalid hex data".to_string()))?;
            req.input.input = Some(input_bytes.into());
        }

        let gas_limit = if let Some(gl) = evm_tx.gas_limit {
            gl
        } else {
            let req = req.clone();
            self.with_provider(move |provider| {
                let req = req.clone();
                async move {
                    provider
                        .estimate_gas(req)
                        .await
                        .map_err(|e| WalletError::GasEstimationFailed(e.to_string()))
                }
            })
            .await?
        };

        // EIP-1559 fee market: prefer alloy's feeHistory-percentile estimate
        // (the ethers-rs/MetaMask algorithm, EIP-1559), falling back to the
        // base-fee × 2 + tip heuristic (audit 4.2) when feeHistory is
        // unavailable, and to legacy gas price when there is no base fee.
        let priority_fee = U256::from(self.priority_fee_wei);
        let (max_fee_per_gas, max_priority_fee_per_gas) = self
            .with_provider(|provider| async move {
                match provider.estimate_eip1559_fees().await {
                    Ok(est) => Ok((
                        Some(est.max_fee_per_gas.to_string()),
                        Some(est.max_priority_fee_per_gas.to_string()),
                    )),
                    Err(_) => {
                        // feeHistory unavailable (e.g. legacy-only RPC):
                        // fall back to the base-fee heuristic.
                        let latest = provider
                            .get_block_by_number(BlockNumberOrTag::Latest)
                            .await
                            .map_err(|e| WalletError::RpcError(e.to_string()))?;
                        match latest.and_then(|b| b.header.base_fee_per_gas) {
                            Some(base_fee) => {
                                let base_fee = U256::from(base_fee);
                                let max_fee = base_fee
                                    .saturating_mul(U256::from(2u64))
                                    .saturating_add(priority_fee);
                                Ok((Some(max_fee.to_string()), Some(priority_fee.to_string())))
                            }
                            None => {
                                let gas_price = provider
                                    .get_gas_price()
                                    .await
                                    .map_err(|e| WalletError::RpcError(e.to_string()))?;
                                Ok((Some(gas_price.to_string()), None))
                            }
                        }
                    }
                }
            })
            .await?;

        let (symbol, _name, decimals) = self.native_asset();
        let per_gas = U256::from_str(max_fee_per_gas.as_deref().unwrap_or("0")).unwrap_or_default();
        let total_wei = per_gas.saturating_mul(U256::from(gas_limit));
        let total_formatted =
            format_units(total_wei, decimals).unwrap_or_else(|_| "0.0".to_string());
        let total = format!("{total_formatted} {symbol}");

        Ok(Fee {
            total,
            currency: symbol,
            details: FeeDetails::Evm {
                gas_limit,
                max_fee_per_gas,
                max_priority_fee_per_gas,
            },
        })
    }

    async fn send_transaction(&self, tx: ChainTransaction) -> Result<TxHash, WalletError> {
        let mut evm_tx = match tx {
            ChainTransaction::Evm(evm_tx) => evm_tx,
            _ => {
                return Err(WalletError::InvalidTransaction(
                    "expected an EVM transaction".to_string(),
                ));
            }
        };
        let raw = self.build_signed_envelope(&mut evm_tx).await?;
        self.broadcast_raw(raw).await
    }

    async fn get_tx_status(&self, tx_hash: &str) -> Result<TxStatus, WalletError> {
        let h = tx_hash.trim_start_matches("0x");
        let bytes = hex::decode(h)
            .map_err(|_| WalletError::InvalidTransaction("Invalid tx hash hex".to_string()))?;
        if bytes.len() != 32 {
            return Err(WalletError::InvalidTransaction(
                "Tx hash must be 32 bytes".to_string(),
            ));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        let b = B256::from(arr);
        let receipt = self
            .with_provider(|provider| async move {
                provider
                    .get_transaction_receipt(b)
                    .await
                    .map_err(|e| WalletError::RpcError(e.to_string()))
            })
            .await?;
        match receipt {
            None => Ok(TxStatus::Pending),
            Some(r) if r.status() => Ok(TxStatus::Confirmed),
            Some(_) => Ok(TxStatus::Failed),
        }
    }

    async fn get_transaction_history(
        &self,
        _address: &str,
        _limit: u32,
    ) -> Result<Vec<TxRecord>, WalletError> {
        // Explorer-backed history is a Phase 2 feature.
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The EIP-20 Transfer topic0 is the canonical keccak of the event
    /// signature (the same hash every ERC-20 emits, e.g. on Etherscan).
    #[test]
    fn transfer_topic0_is_canonical() {
        let canonical: B256 = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
            .parse()
            .unwrap();
        assert_eq!(transfer_topic0(), canonical);
    }

    /// An adapter whose endpoints are never actually contacted (Http connects
    /// lazily per request), so the fallback loop can be tested offline.
    async fn offline_adapter(fallbacks: &[&str]) -> EvmAdapter {
        let urls: Vec<String> = fallbacks.iter().map(|u| u.to_string()).collect();
        EvmAdapter::new("https://127.0.0.1:1", 1, "test", &urls)
            .await
            .unwrap()
    }

    #[test]
    fn transport_failure_classification() {
        // These are retried against fallback endpoints…
        for e in [
            WalletError::RpcError("x".into()),
            WalletError::NetworkError("x".into()),
            WalletError::GasEstimationFailed("x".into()),
            WalletError::TransactionFailed("x".into()),
        ] {
            assert!(EvmAdapter::is_transport_failure(&e), "{e:?}");
        }
        // …while local errors fail fast.
        for e in [
            WalletError::InvalidAmount("x".into()),
            WalletError::InvalidTransaction("x".into()),
            WalletError::SigningFailed("x".into()),
            WalletError::AccountNotFound("x".into()),
        ] {
            assert!(!EvmAdapter::is_transport_failure(&e), "{e:?}");
        }
    }

    #[tokio::test]
    async fn with_provider_returns_first_success() {
        let adapter = offline_adapter(&["https://127.0.0.1:2"]).await;
        let value = adapter
            .with_provider(|_p| async move { Ok::<u32, WalletError>(42) })
            .await
            .unwrap();
        assert_eq!(value, 42);
    }

    #[tokio::test]
    async fn with_provider_returns_last_transport_error_when_all_fail() {
        let adapter = offline_adapter(&["https://127.0.0.1:2"]).await;
        let err = adapter
            .with_provider(|_p| async move {
                Err::<u32, WalletError>(WalletError::RpcError("down".into()))
            })
            .await
            .unwrap_err();
        assert!(matches!(err, WalletError::RpcError(m) if m == "down"));
    }

    #[tokio::test]
    async fn with_provider_fails_fast_on_local_errors() {
        let adapter = offline_adapter(&["https://127.0.0.1:2"]).await;
        let err = adapter
            .with_provider(|_p| async move {
                Err::<u32, WalletError>(WalletError::InvalidAmount("x".into()))
            })
            .await
            .unwrap_err();
        assert!(matches!(err, WalletError::InvalidAmount(_)));
    }
}
