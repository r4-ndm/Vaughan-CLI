//! Pluggable on-chain balance source for sizing signer spend caps.
//!
//! Every policy cap (`daily_cap_pct`, `max_position_pct`, `max_approve_pct`) and
//! the spend ledger's per-asset budget are a percentage of the REAL balance of
//! the active account. [`BalanceSource`] is the single seam that supplies that
//! number: [`LiveBalanceSource`] reads it from chain, [`MockBalanceSource`]
//! supplies a deterministic value for tests. There is no third option — a
//! caller that cannot get a real answer gets an `Err`, never a default.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;
use async_trait::async_trait;

sol! {
    interface IERC20BalanceProbe {
        function balanceOf(address account) external view returns (uint256);
    }
}

/// Upper bound on a single RPC round trip; a hung node must not hang the daemon.
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Resolves the on-chain balance that signer policy caps are sized against.
///
/// `token: None` means the chain's native asset (`eth_getBalance`); `Some(addr)`
/// means the ERC-20 `balanceOf(owner)` of that token contract.
#[async_trait]
pub trait BalanceSource: Send + Sync {
    async fn balance_of(
        &self,
        rpc_url: &str,
        owner: Address,
        token: Option<Address>,
    ) -> Result<U256, String>;
}

/// Live balance source: `eth_getBalance` for native, `balanceOf` `eth_call` for ERC-20.
pub struct LiveBalanceSource;

#[async_trait]
impl BalanceSource for LiveBalanceSource {
    async fn balance_of(
        &self,
        rpc_url: &str,
        owner: Address,
        token: Option<Address>,
    ) -> Result<U256, String> {
        let url: alloy::transports::http::reqwest::Url = rpc_url
            .parse()
            .map_err(|e| format!("invalid rpc_url: {e}"))?;
        let provider = ProviderBuilder::new().connect_http(url);

        match token {
            None => tokio::time::timeout(RPC_TIMEOUT, provider.get_balance(owner))
                .await
                .map_err(|_| "eth_getBalance timed out".to_string())?
                .map_err(|e| format!("eth_getBalance failed: {e}")),
            Some(token) => {
                let input = IERC20BalanceProbe::balanceOfCall { account: owner }.abi_encode();
                let req = TransactionRequest::default().to(token).input(input.into());
                let raw = tokio::time::timeout(RPC_TIMEOUT, provider.call(req))
                    .await
                    .map_err(|_| format!("balanceOf({token:#x}) eth_call timed out"))?
                    .map_err(|e| format!("balanceOf({token:#x}) eth_call failed: {e}"))?;
                if raw.len() != 32 {
                    return Err(format!(
                        "balanceOf({token:#x}) returned {} bytes, expected 32 (not an ERC-20 contract?)",
                        raw.len()
                    ));
                }
                IERC20BalanceProbe::balanceOfCall::abi_decode_returns(&raw)
                    .map_err(|e| format!("bad balanceOf({token:#x}) return: {e}"))
            }
        }
    }
}

/// Deterministic balance source for unit/integration tests.
///
/// Lookup order: fail switch, then a per-`(owner, token)` override, then a
/// per-`token` override (owner-agnostic — matches the common case where every
/// call in a test uses the signer's one active account), then the default.
#[derive(Debug)]
pub struct MockBalanceSource {
    default: Mutex<U256>,
    by_token: Mutex<HashMap<Option<Address>, U256>>,
    by_owner_token: Mutex<HashMap<(Address, Option<Address>), U256>>,
    should_fail: Mutex<bool>,
    fail_message: Mutex<String>,
    calls: Mutex<Vec<(Address, Option<Address>)>>,
}

impl Default for MockBalanceSource {
    fn default() -> Self {
        Self {
            // Generous default so tests that never call a setter keep their
            // existing meaning (this mirrors the old hardcoded 10-unit balance).
            default: Mutex::new(U256::from(10_000_000_000_000_000_000u128)),
            by_token: Mutex::new(HashMap::new()),
            by_owner_token: Mutex::new(HashMap::new()),
            should_fail: Mutex::new(false),
            fail_message: Mutex::new("balance RPC unreachable".to_string()),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl MockBalanceSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the fallback balance returned when no more specific override matches.
    pub fn set_default(&self, balance: U256) {
        *self.default.lock().unwrap() = balance;
    }

    /// Override the balance for `token` regardless of which owner is asked about.
    pub fn set(&self, token: Option<Address>, balance: U256) {
        self.by_token.lock().unwrap().insert(token, balance);
    }

    /// Override the balance for one specific `(owner, token)` pair (e.g. a stealth address).
    pub fn set_for_owner(&self, owner: Address, token: Option<Address>, balance: U256) {
        self.by_owner_token
            .lock()
            .unwrap()
            .insert((owner, token), balance);
    }

    /// Make every subsequent `balance_of` call fail with `message`.
    pub fn set_fail(&self, fail: bool) {
        *self.should_fail.lock().unwrap() = fail;
    }

    pub fn set_fail_message(&self, msg: impl Into<String>) {
        *self.fail_message.lock().unwrap() = msg.into();
    }

    /// Recorded `(owner, token)` arguments from every `balance_of` call, in order.
    pub fn calls(&self) -> Vec<(Address, Option<Address>)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl BalanceSource for MockBalanceSource {
    async fn balance_of(
        &self,
        _rpc_url: &str,
        owner: Address,
        token: Option<Address>,
    ) -> Result<U256, String> {
        self.calls.lock().unwrap().push((owner, token));

        if *self.should_fail.lock().unwrap() {
            return Err(self.fail_message.lock().unwrap().clone());
        }
        if let Some(b) = self.by_owner_token.lock().unwrap().get(&(owner, token)) {
            return Ok(*b);
        }
        if let Some(b) = self.by_token.lock().unwrap().get(&token) {
            return Ok(*b);
        }
        Ok(*self.default.lock().unwrap())
    }
}
