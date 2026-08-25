//! Vaughan Contract Browser & DEX Engine (`wiz4rd-engine`).
//!
//! A modular, protocol-agnostic EVM smart contract introspection and
//! interaction engine:
//! - [`abi`]: Explorer ABI fetching & local disk caching.
//! - [`selectors`]: Bytecode `PUSH4` candidate function selector extraction.
//! - [`sigdb`]: 4byte.directory reverse signature lookup.
//! - [`call`]: Dynamic function call execution (`alloy-dyn-abi`).
//! - [`probe`]: Standard capability & protocol fingerprinting.
//! - [`events`]: Factory pair/pool indexing and log scanning.

pub mod abi;
pub mod call;
pub mod events;
pub mod probe;
pub mod selectors;
pub mod sigdb;

use abi::{AbiResolution, AbiResolver};
use alloy::json_abi::JsonAbi;
use alloy::primitives::{Address, Bytes};
use alloy::providers::Provider;
use call::{CallResult, DynamicCaller};
use probe::{ContractFingerprint, ContractProber};
use selectors::Selector;
use sigdb::SignatureDb;
use std::sync::Arc;

/// Comprehensive contract inspection summary.
#[derive(Debug, Clone)]
pub struct ContractInspection {
    pub address: Address,
    pub chain_id: u64,
    pub fingerprint: ContractFingerprint,
    pub abi_resolution: AbiResolution,
    pub candidate_selectors: Vec<Selector>,
}

/// Unified Contract Browser Engine.
#[derive(Debug, Clone)]
pub struct BrowserEngine {
    pub abi_resolver: AbiResolver,
    pub sig_db: Arc<SignatureDb>,
}

impl Default for BrowserEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserEngine {
    /// Create a new BrowserEngine instance with default resolvers.
    pub fn new() -> Self {
        Self {
            abi_resolver: AbiResolver::new(),
            sig_db: Arc::new(SignatureDb::new()),
        }
    }

    /// Full inspection of a smart contract at `address`.
    pub async fn inspect<P: Provider>(
        &self,
        provider: &P,
        chain_id: u64,
        address: Address,
    ) -> ContractInspection {
        // 1. Probe capability fingerprint
        let fingerprint = ContractProber::probe(provider, address).await;

        // 2. Resolve ABI
        let abi_res = self.abi_resolver.resolve(chain_id, address).await;

        // 3. Extract candidate bytecode selectors if not verified
        let candidate_selectors = if !matches!(abi_res, AbiResolution::Verified(_)) {
            let code = provider.get_code_at(address).await.unwrap_or_default();
            selectors::extract_selectors(&code)
        } else {
            Vec::new()
        };

        ContractInspection {
            address,
            chain_id,
            fingerprint,
            abi_resolution: abi_res,
            candidate_selectors,
        }
    }

    /// Encode calldata for a named ABI function (no RPC). Used by gated writes.
    pub fn encode_named(
        abi: &JsonAbi,
        function_name: &str,
        args: &[String],
    ) -> Result<Bytes, String> {
        let func = abi
            .functions
            .get(function_name)
            .and_then(|funcs| funcs.first())
            .ok_or_else(|| format!("Function '{function_name}' not found in contract ABI"))?;
        DynamicCaller::encode_call(func, args)
    }

    /// Execute a dynamic read-only call on a contract function by name.
    pub async fn call_named<P: Provider>(
        &self,
        provider: &P,
        target: Address,
        abi: &JsonAbi,
        function_name: &str,
        args: &[String],
    ) -> Result<CallResult, String> {
        let func = abi
            .functions
            .get(function_name)
            .and_then(|funcs| funcs.first())
            .ok_or_else(|| format!("Function '{function_name}' not found in contract ABI"))?;

        DynamicCaller::call_function(provider, target, func, args).await
    }

    /// Execute a raw `eth_call`.
    pub async fn call_raw<P: Provider>(
        &self,
        provider: &P,
        target: Address,
        calldata: Bytes,
    ) -> Result<Bytes, String> {
        DynamicCaller::call_raw(provider, target, calldata).await
    }
}
