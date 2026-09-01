use alloy::eips::eip2718::Encodable2718;
use alloy::network::{Ethereum, EthereumWallet, NetworkTransactionBuilder};
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::eth::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use url::Url;

use crate::error::AgentError;
use crate::sentient::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use crate::sentient::quorum::QuorumValidator;

/// Result of an autonomous swap attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapExecution {
    /// Broadcast hash, or [`B256::ZERO`] when [`Self::dry_run`] is set.
    pub tx_hash: B256,
    /// When true, simulation + breakers ran but nothing was broadcast.
    pub dry_run: bool,
}

/// Autonomous trader running inside an isolated burner wallet profile.
pub struct SentientTrader {
    signer: PrivateKeySigner,
    circuit_breaker: CircuitBreaker,
    rpc_urls: Vec<String>,
    chain_id: u64,
    /// When true, validate + simulate but never broadcast (safe paper trading).
    dry_run: bool,
}

impl SentientTrader {
    pub fn new(
        signer: PrivateKeySigner,
        rpc_urls: Vec<String>,
        chain_id: u64,
        breaker_config: CircuitBreakerConfig,
    ) -> Self {
        Self {
            signer,
            circuit_breaker: CircuitBreaker::new(breaker_config),
            rpc_urls,
            chain_id,
            dry_run: dry_run_from_env(),
        }
    }

    /// Enable or disable dry-run (overrides env default).
    pub fn with_dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// Wallet address of the dedicated burner signer.
    pub fn address(&self) -> Address {
        self.signer.address()
    }

    /// Access the circuit breaker state.
    pub fn circuit_breaker(&self) -> &CircuitBreaker {
        &self.circuit_breaker
    }

    /// Execute a DEX swap autonomously with multi-RPC quorum validation and circuit breakers.
    ///
    /// In dry-run mode, steps 1–4 still run; step 5 returns [`SwapExecution`] with
    /// `dry_run: true` and a zero hash (nothing is signed or broadcast).
    pub async fn execute_swap(
        &self,
        router: Address,
        pair: Option<Address>,
        calldata: Bytes,
        value_wei: U256,
        trade_amount: U256,
        slippage_bps: u32,
    ) -> Result<SwapExecution, AgentError> {
        if self.rpc_urls.is_empty() {
            return Err(AgentError::InvalidToolCall(
                "No RPC endpoints configured".to_string(),
            ));
        }

        if !vaughan_core::core::is_allowed_dex_router(self.chain_id, router) {
            return Err(AgentError::InvalidToolCall(format!(
                "router {router:#x} is not on the Pulse DEX allowlist for chain {} — refusing swap",
                self.chain_id
            )));
        }

        let primary_url = Url::parse(&self.rpc_urls[0])
            .map_err(|e| AgentError::InvalidToolCall(format!("Invalid primary RPC URL: {e}")))?;

        let provider: RootProvider<Ethereum> = RootProvider::new_http(primary_url);

        // 1. Check native balance for position sizing check
        let balance = provider
            .get_balance(self.address())
            .await
            .unwrap_or(U256::ZERO);

        // 2. Validate against circuit breaker rules
        self.circuit_breaker
            .validate_trade(trade_amount, balance, slippage_bps)?;

        // 3. Multi-RPC quorum validation if pair address provided
        if let Some(pair_addr) = pair {
            let need = self.circuit_breaker.config().required_rpc_quorum.max(1);
            if self.rpc_urls.len() >= need && need >= 2 {
                QuorumValidator::validate_pair_reserves(&self.rpc_urls, pair_addr, need).await?;
            } else if need >= 2 {
                tracing::warn!(
                    target: "vaughan_agent::sentient",
                    configured = self.rpc_urls.len(),
                    required = need,
                    "multi-RPC reserve quorum skipped — add more RPC URLs to the Sentient profile"
                );
            }
        }

        // 4. Pre-flight simulation (eth_call ignores gas budget)
        let tx_req = TransactionRequest::default()
            .from(self.address())
            .to(router)
            .input(calldata.clone().into())
            .value(value_wei);

        if let Err(e) = provider.call(tx_req.clone()).await {
            self.circuit_breaker
                .record_failure(&format!("Simulation failed: {e}"));
            return Err(AgentError::ProviderError(format!(
                "Pre-flight swap simulation reverted: {e}"
            )));
        }

        // 5. Fees — PulseChain RPCs often report inflated eth_gasPrice; cap tips.
        let (max_fee_per_gas, max_priority_fee_per_gas, gas_limit) =
            self.suggest_fees(&provider).await;
        let gas_budget = U256::from(gas_limit).saturating_mul(U256::from(max_fee_per_gas));

        // Native swaps: value + max gas must fit in balance (eth_call won't catch this).
        if value_wei.saturating_add(gas_budget) > balance {
            let max_value = balance.saturating_sub(gas_budget);
            return Err(AgentError::InvalidToolCall(format!(
                "insufficient funds for swap value + gas: balance={balance} wei, value={value_wei} wei, \
                 gas_budget={gas_budget} wei. Reduce amount_in to ≤ {max_value} wei and retry \
                 (leave headroom for gas; session still open)"
            )));
        }

        if self.dry_run {
            self.circuit_breaker.record_success(gas_budget)?;
            return Ok(SwapExecution {
                tx_hash: B256::ZERO,
                dry_run: true,
            });
        }

        // 6. Build, sign, and broadcast
        let nonce = provider
            .get_transaction_count(self.address())
            .await
            .map_err(|e| AgentError::ProviderError(format!("Failed to get nonce: {e}")))?;

        let mut tx = TransactionRequest::default()
            .from(self.address())
            .to(router)
            .input(calldata.into())
            .value(value_wei);

        tx.nonce = Some(nonce);
        tx.gas = Some(gas_limit);
        tx.max_fee_per_gas = Some(max_fee_per_gas);
        tx.max_priority_fee_per_gas = Some(max_priority_fee_per_gas);
        tx.chain_id = Some(self.chain_id);

        let wallet = EthereumWallet::from(self.signer.clone());
        let signed_tx = tx.build(&wallet).await.map_err(|e| {
            AgentError::SecurityViolation(format!("Failed to sign autonomous tx: {e}"))
        })?;

        let raw_tx = signed_tx.encoded_2718();
        let pending_tx = provider.send_raw_transaction(&raw_tx).await.map_err(|e| {
            self.circuit_breaker
                .record_failure(&format!("Broadcast failed: {e}"));
            AgentError::ProviderError(format!("Broadcast failed: {e}"))
        })?;

        let tx_hash = *pending_tx.tx_hash();

        self.circuit_breaker.record_success(gas_budget)?;

        Ok(SwapExecution {
            tx_hash,
            dry_run: false,
        })
    }

    /// EIP-1559 fee suggestion with PulseChain-safe caps (avoid eth_gasPrice blow-ups).
    async fn suggest_fees(&self, provider: &RootProvider<Ethereum>) -> (u128, u128, u64) {
        const GAS_LIMIT: u64 = 350_000;
        // PulseChain mainnet/testnet tips are ~0.01 gwei; RPCs sometimes return absurd gasPrice.
        const PLS_TIP: u128 = 10_000_000; // 0.01 gwei
        const PLS_MAX_FEE_CAP: u128 = 100_000_000; // 0.1 gwei ceiling
        const DEFAULT_MAX_FEE: u128 = 2_000_000_000; // 2 gwei elsewhere

        let raw = provider.get_gas_price().await.unwrap_or(1_000_000_000);

        if matches!(self.chain_id, 369 | 943) {
            let max_fee = (raw as u128)
                .saturating_mul(2)
                .clamp(PLS_TIP, PLS_MAX_FEE_CAP);
            (max_fee, PLS_TIP.min(max_fee), GAS_LIMIT)
        } else {
            let max_fee = (raw as u128).saturating_mul(2).max(DEFAULT_MAX_FEE);
            let tip = (raw as u128).min(max_fee);
            (max_fee, tip, GAS_LIMIT)
        }
    }
}

/// `VAUGHAN_SENTIENT_DRY_RUN=1|true` enables paper trading (no broadcast).
/// Legacy env `VAUGHAN_DEGEN_DRY_RUN` is still accepted.
pub fn dry_run_from_env() -> bool {
    fn truthy(key: &str) -> bool {
        match std::env::var(key) {
            Ok(v) => {
                let v = v.trim();
                v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes")
            }
            Err(_) => false,
        }
    }
    truthy("VAUGHAN_SENTIENT_DRY_RUN") || truthy("VAUGHAN_DEGEN_DRY_RUN")
}
