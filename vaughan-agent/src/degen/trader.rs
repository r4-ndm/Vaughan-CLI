use alloy::eips::eip2718::Encodable2718;
use alloy::network::{Ethereum, EthereumWallet, NetworkTransactionBuilder};
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::eth::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use url::Url;

use crate::degen::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use crate::degen::quorum::QuorumValidator;
use crate::error::AgentError;

/// Autonomous trader running inside an isolated burner wallet profile.
pub struct DegenTrader {
    signer: PrivateKeySigner,
    circuit_breaker: CircuitBreaker,
    rpc_urls: Vec<String>,
    chain_id: u64,
}

impl DegenTrader {
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
        }
    }

    /// Wallet address of the dedicated burner signer.
    pub fn address(&self) -> Address {
        self.signer.address()
    }

    /// Access the circuit breaker state.
    pub fn circuit_breaker(&self) -> &CircuitBreaker {
        &self.circuit_breaker
    }

    /// Trigger immediate emergency stop.
    pub fn emergency_stop(&self, reason: impl Into<String>) {
        self.circuit_breaker.trip(reason);
    }

    /// Execute a DEX swap autonomously with multi-RPC quorum validation and circuit breakers.
    pub async fn execute_swap(
        &self,
        router: Address,
        pair: Option<Address>,
        calldata: Bytes,
        value_wei: U256,
        trade_amount: U256,
        slippage_bps: u32,
    ) -> Result<alloy::primitives::TxHash, AgentError> {
        if self.rpc_urls.is_empty() {
            return Err(AgentError::InvalidToolCall(
                "No RPC endpoints configured".to_string(),
            ));
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

        // 3. Multi-RPC quorum validation if pair address provided and multiple RPCs available
        if let Some(pair_addr) = pair {
            if self.rpc_urls.len() >= 2 {
                QuorumValidator::validate_pair_reserves(&self.rpc_urls, pair_addr, 2).await?;
            }
        }

        // 4. Pre-flight simulation
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

        // 5. Build, sign, and broadcast
        let nonce = provider
            .get_transaction_count(self.address())
            .await
            .map_err(|e| AgentError::ProviderError(format!("Failed to get nonce: {e}")))?;

        let gas_price = provider.get_gas_price().await.unwrap_or(1_000_000_000); // 1 gwei fallback

        let mut tx = TransactionRequest::default()
            .from(self.address())
            .to(router)
            .input(calldata.into())
            .value(value_wei);

        tx.nonce = Some(nonce);
        tx.gas = Some(300_000);
        tx.max_fee_per_gas = Some(gas_price as u128 * 2);
        tx.max_priority_fee_per_gas = Some(gas_price as u128);
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

        // 6. Record gas expenditure
        let gas_spent = U256::from(300_000 * gas_price * 2);
        self.circuit_breaker.record_success(gas_spent)?;

        Ok(tx_hash)
    }
}
