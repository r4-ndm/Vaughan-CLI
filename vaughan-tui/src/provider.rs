//! The bridge between the local EIP-1193 provider server and the TUI.
//!
//! `vaughan-provider` is transport-only: it hands every request to a host
//! [`WalletHandle`]. This module implements that handle for the TUI by
//! forwarding each request to the UI thread over an MPSC channel and awaiting
//! a `oneshot` reply. The UI owns the [`WalletState`] exclusively, so **all**
//! wallet access — even read-only queries — funnels through this one channel
//! and runs on the UI thread. Signing requests become an approval prompt; the
//! handler future simply blocks on the user's answer.
//!
//! This keeps key material on the UI thread (no shared mutable wallet state)
//! and guarantees no signing happens without the UI showing the request first.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};

use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::{format_base_units, WalletState};
use vaughan_core::error::WalletError;
use vaughan_provider::server::DEFAULT_PORT;
use vaughan_provider::{
    Eip1193Handler, EventBus, ProviderError, ProviderServer, RequestCtx, TxParams, WalletHandle,
};

/// A signing/sending request that requires a fresh, explicit user approval.
#[derive(Debug, Clone)]
pub enum ApprovalKind {
    /// Sign and broadcast; returns the tx hash.
    SendTransaction(TxParams),
    /// Sign only; returns the raw signed tx (for the browser signer backend).
    SignTransaction(TxParams),
    /// EIP-191 personal message signing.
    SignMessage { address: String, message: String },
    /// EIP-712 typed-data signing.
    SignTypedData { address: String, typed_data: Value },
}

/// A request forwarded from the provider handler to the UI thread.
pub enum HostRequest {
    /// `eth_accounts`: the caller-visible account list.
    Accounts {
        reply: oneshot::Sender<Result<Vec<String>, ProviderError>>,
    },
    /// `eth_requestAccounts`: connect gesture, same answer as `Accounts`.
    RequestAccounts {
        reply: oneshot::Sender<Result<Vec<String>, ProviderError>>,
    },
    /// `eth_chainId`: the active chain id as `0x` hex.
    ChainId {
        reply: oneshot::Sender<Result<String, ProviderError>>,
    },
    /// `wallet_switchEthereumChain`: switch to a built-in network.
    SwitchChain {
        chain_id: String,
        reply: oneshot::Sender<Result<(), ProviderError>>,
    },
    /// A sign/send request needing user approval.
    Approval {
        // Boxed: `ApprovalKind` (which embeds `TxParams`) is much larger than
        // the other variants, so boxing keeps the whole enum small.
        kind: Box<ApprovalKind>,
        origin: Option<String>,
        reply: oneshot::Sender<Result<String, ProviderError>>,
    },
}

/// The [`WalletHandle`] handed to the provider server. Cheaply cloneable via
/// the shared request channel; the UI thread is the single consumer.
pub struct ProviderHost {
    requests: mpsc::UnboundedSender<HostRequest>,
}

impl ProviderHost {
    pub fn new(requests: mpsc::UnboundedSender<HostRequest>) -> Self {
        Self { requests }
    }

    fn disconnected(_: tokio::sync::mpsc::error::SendError<HostRequest>) -> ProviderError {
        ProviderError::Disconnected("wallet UI is closed".to_string())
    }

    fn dropped(_: oneshot::error::RecvError) -> ProviderError {
        ProviderError::Disconnected("wallet UI is closed".to_string())
    }

    /// Queue a sign/send request and await the UI's decision/result.
    async fn approve(&self, kind: ApprovalKind, ctx: &RequestCtx) -> Result<String, ProviderError> {
        let (reply, rx) = oneshot::channel();
        self.requests
            .send(HostRequest::Approval {
                kind: Box::new(kind),
                origin: ctx.origin.clone(),
                reply,
            })
            .map_err(Self::disconnected)?;
        rx.await.map_err(Self::dropped)?
    }
}

#[async_trait]
impl WalletHandle for ProviderHost {
    async fn accounts(&self, _ctx: &RequestCtx) -> Result<Vec<String>, ProviderError> {
        let (reply, rx) = oneshot::channel();
        self.requests
            .send(HostRequest::Accounts { reply })
            .map_err(Self::disconnected)?;
        rx.await.map_err(Self::dropped)?
    }

    async fn request_accounts(&self, _ctx: &RequestCtx) -> Result<Vec<String>, ProviderError> {
        let (reply, rx) = oneshot::channel();
        self.requests
            .send(HostRequest::RequestAccounts { reply })
            .map_err(Self::disconnected)?;
        rx.await.map_err(Self::dropped)?
    }

    async fn chain_id(&self, _ctx: &RequestCtx) -> Result<String, ProviderError> {
        let (reply, rx) = oneshot::channel();
        self.requests
            .send(HostRequest::ChainId { reply })
            .map_err(Self::disconnected)?;
        rx.await.map_err(Self::dropped)?
    }

    async fn send_transaction(
        &self,
        ctx: &RequestCtx,
        tx: TxParams,
    ) -> Result<String, ProviderError> {
        self.approve(ApprovalKind::SendTransaction(tx), ctx).await
    }

    async fn sign_transaction(
        &self,
        ctx: &RequestCtx,
        tx: TxParams,
    ) -> Result<String, ProviderError> {
        self.approve(ApprovalKind::SignTransaction(tx), ctx).await
    }

    async fn sign_message(
        &self,
        ctx: &RequestCtx,
        address: &str,
        message: &str,
    ) -> Result<String, ProviderError> {
        self.approve(
            ApprovalKind::SignMessage {
                address: address.to_string(),
                message: message.to_string(),
            },
            ctx,
        )
        .await
    }

    async fn sign_typed_data(
        &self,
        ctx: &RequestCtx,
        address: &str,
        typed_data: Value,
    ) -> Result<String, ProviderError> {
        self.approve(
            ApprovalKind::SignTypedData {
                address: address.to_string(),
                typed_data,
            },
            ctx,
        )
        .await
    }

    async fn switch_chain(&self, _ctx: &RequestCtx, chain_id: &str) -> Result<(), ProviderError> {
        let (reply, rx) = oneshot::channel();
        self.requests
            .send(HostRequest::SwitchChain {
                chain_id: chain_id.to_string(),
                reply,
            })
            .map_err(Self::disconnected)?;
        rx.await.map_err(Self::dropped)?
    }
}

/// Bind the loopback provider server and serve it in the background.
///
/// The server owns nothing wallet-related; it forwards through
/// [`ProviderHost`] into `requests` and relays `events` to clients. A bind
/// failure (e.g. port already in use) is logged, not fatal — the wallet still
/// works without the dApp bridge.
pub fn spawn_provider_server(
    handle: &Handle,
    requests: mpsc::UnboundedSender<HostRequest>,
    events: EventBus,
) {
    handle.spawn(async move {
        let server = match ProviderServer::bind(DEFAULT_PORT).await {
            Ok(server) => server,
            Err(e) => {
                tracing::warn!(error = %e, "provider server failed to bind; dApp bridge disabled");
                return;
            }
        };
        tracing::info!(url = %server.url(), "provider server listening");
        let handler = Arc::new(Eip1193Handler::new(Arc::new(ProviderHost::new(requests))));
        if let Err(e) = server.serve(handler, events).await {
            tracing::warn!(error = %e, "provider server stopped");
        }
    });
}

/// Render a short, human-readable summary of `kind` for the approval prompt.
///
/// Returns `(title, details)` where `details` is shown verbatim. This is the
/// "show the user the full request" step of the signing guardrail; no secret
/// material is involved (these are the request fields the dApp sent).
///
/// TODO: transactions omit the fee from the prompt today — it is estimated and
/// applied at execution time. Fully satisfying guardrail #4 ("…and fee") means
/// estimating the fee before the prompt is shown, which needs a network call
/// at approval-setup time.
pub fn describe_approval(kind: &ApprovalKind, wallet: &WalletState) -> (String, Vec<String>) {
    match kind {
        ApprovalKind::SendTransaction(tx) => (
            "Sign & broadcast transaction".into(),
            describe_tx(tx, wallet),
        ),
        ApprovalKind::SignTransaction(tx) => (
            "Sign transaction (no broadcast)".into(),
            describe_tx(tx, wallet),
        ),
        ApprovalKind::SignMessage { message, .. } => (
            "Sign message (personal_sign)".into(),
            vec![
                "Method:  personal_sign".to_string(),
                format!("Message: {message}"),
            ],
        ),
        ApprovalKind::SignTypedData { typed_data, .. } => {
            let primary = typed_data["primaryType"].as_str().unwrap_or("?");
            let domain = typed_data["domain"]["name"].as_str().unwrap_or("?");
            (
                "Sign typed data (eth_signTypedData_v4)".into(),
                vec![
                    "Method:  eth_signTypedData_v4".to_string(),
                    format!("Domain:  {domain}"),
                    format!("Type:    {primary}"),
                ],
            )
        }
    }
}

/// Describe a transaction's recipient, value, chain, and data.
fn describe_tx(tx: &TxParams, wallet: &WalletState) -> Vec<String> {
    let net = wallet.networks().active();
    let to = tx.to.as_deref().unwrap_or("(contract create)");
    let value = format_base_units(tx.value.as_deref().unwrap_or("0"), net.decimals);
    let testnet = if net.is_testnet { " (testnet)" } else { "" };
    let mut lines = vec![
        format!("To:      {to}"),
        format!("Value:   {value} {}", net.native_symbol),
        format!("Network: {}{testnet}", net.name),
    ];
    if let Some(data) = tx.data.as_deref() {
        lines.push(format!("Data:    {data}"));
    }
    lines
}

/// Execute an approved request, producing the value the handler returns.
///
/// Runs on the UI thread; `handle.block_on` is used for the two network-backed
/// paths (sign-only raw tx and broadcast), matching the existing view pattern.
pub fn execute_approval(
    kind: &ApprovalKind,
    wallet: &WalletState,
    handle: &Handle,
) -> Result<String, ProviderError> {
    match kind {
        ApprovalKind::SignMessage { address, message } => {
            verify_address(address, wallet)?;
            wallet
                .sign_message(&decode_message(message)?)
                .map_err(map_wallet_error)
        }
        ApprovalKind::SignTypedData {
            address,
            typed_data,
        } => {
            verify_address(address, wallet)?;
            wallet.sign_typed_data(typed_data).map_err(map_wallet_error)
        }
        ApprovalKind::SignTransaction(tx) => {
            let evm = to_evm_transaction(tx, wallet)?;
            handle
                .block_on(wallet.sign_transaction(evm))
                .map_err(map_wallet_error)
        }
        ApprovalKind::SendTransaction(tx) => {
            let evm = to_evm_transaction(tx, wallet)?;
            handle
                .block_on(wallet.send_transaction(evm))
                .map(|hash| hash.to_string())
                .map_err(map_wallet_error)
        }
    }
}

/// Map [`TxParams`] (already quantity-normalized by the provider) onto an
/// [`EvmTransaction`]. `from` defaults to the active account and `value` to
/// `"0"`; `chain_id` is taken from the active network, and `to` is required
/// (contract creation is not supported yet).
fn to_evm_transaction(
    tx: &TxParams,
    wallet: &WalletState,
) -> Result<EvmTransaction, ProviderError> {
    let net = wallet.networks().active();
    let from = tx
        .from
        .clone()
        .unwrap_or_else(|| wallet.active_address().unwrap_or("").to_string());
    let to = tx
        .to
        .clone()
        .ok_or_else(|| ProviderError::InvalidParams("transaction has no `to` address".into()))?;
    Ok(EvmTransaction {
        from,
        to,
        value: tx.value.clone().unwrap_or_else(|| "0".to_string()),
        data: tx.data.clone(),
        gas_limit: parse_optional_u64(tx.gas.as_deref(), "gas")?,
        gas_price: tx.gas_price.clone(),
        max_fee_per_gas: tx.max_fee_per_gas.clone(),
        max_priority_fee_per_gas: tx.max_priority_fee_per_gas.clone(),
        nonce: parse_optional_u64(tx.nonce.as_deref(), "nonce")?,
        chain_id: net.chain_id,
    })
}

fn parse_optional_u64(value: Option<&str>, field: &str) -> Result<Option<u64>, ProviderError> {
    value
        .map(|v| {
            v.parse::<u64>()
                .map_err(|_| ProviderError::InvalidParams(format!("invalid {field}: {v}")))
        })
        .transpose()
}

/// Decode a `personal_sign` message: `0x`-prefixed hex is decoded to bytes,
/// anything else is signed as UTF-8.
fn decode_message(message: &str) -> Result<Vec<u8>, ProviderError> {
    if let Some(hex) = message.strip_prefix("0x") {
        hex::decode(hex)
            .map_err(|_| ProviderError::InvalidParams(format!("invalid hex message: {message}")))
    } else {
        Ok(message.as_bytes().to_vec())
    }
}

/// The request must target the active account; signing with a different
/// account silently would mirror the browser's "wrong device" failure.
fn verify_address(address: &str, wallet: &WalletState) -> Result<(), ProviderError> {
    let active = wallet.active_address().map_err(map_wallet_error)?;
    if address.eq_ignore_ascii_case(active) {
        Ok(())
    } else {
        Err(ProviderError::Unauthorized(format!(
            "account {address} is not the active account"
        )))
    }
}

fn map_wallet_error(e: WalletError) -> ProviderError {
    ProviderError::Internal(e.user_message())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vaughan_core::chains::evm::networks::get_network_by_chain_id;

    #[test]
    fn decode_message_handles_hex_and_utf8() {
        assert_eq!(decode_message("0x68656c6c6f").unwrap(), b"hello");
        assert_eq!(decode_message("hello").unwrap(), b"hello");
        assert!(decode_message("0xzz").is_err());
    }

    #[test]
    fn parse_optional_u64_handles_values() {
        assert_eq!(parse_optional_u64(None, "gas").unwrap(), None);
        assert_eq!(
            parse_optional_u64(Some("21000"), "gas").unwrap(),
            Some(21_000)
        );
        assert!(parse_optional_u64(Some("abc"), "gas").is_err());
    }

    #[test]
    fn to_evm_transaction_requires_to() {
        // Needs an unlocked wallet only for the active context; a locked wallet
        // fails later, but the `to` check must fire first and be deterministic.
        let wallet = WalletState::load(tempfile::tempdir().unwrap().path().join("w.json")).unwrap();
        let missing_to = TxParams {
            from: None,
            to: None,
            data: None,
            value: None,
            gas: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id: None,
        };
        assert!(matches!(
            to_evm_transaction(&missing_to, &wallet),
            Err(ProviderError::InvalidParams(_))
        ));
    }

    #[test]
    fn describe_sign_message_is_non_empty() {
        // A locked/uninitialized wallet still yields a useful prompt for
        // message signing (address/chain aren't shown there).
        let wallet = WalletState::load(tempfile::tempdir().unwrap().path().join("w.json")).unwrap();
        let kind = ApprovalKind::SignMessage {
            address: "0xabc".into(),
            message: "hello".into(),
        };
        let (title, details) = describe_approval(&kind, &wallet);
        assert_eq!(title, "Sign message (personal_sign)");
        assert!(details.iter().any(|l| l.contains("hello")));
    }

    #[test]
    fn chain_id_hex_roundtrip() {
        // Sanity check the chain-id formatting used by the UI (`0x{id:x}`).
        let net = get_network_by_chain_id(369).unwrap();
        assert_eq!(format!("0x{:x}", net.chain_id), "0x171");
    }
}
