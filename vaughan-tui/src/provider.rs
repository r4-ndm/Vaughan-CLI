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

use std::env;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};

use vaughan_core::chains::{EvmTransaction, Fee};
use vaughan_core::core::{format_base_units, WalletState};
use vaughan_core::error::WalletError;
use vaughan_provider::server::DEFAULT_PORT;
use vaughan_provider::{
    Eip1193Handler, EventBus, ProviderError, ProviderServer, RequestCtx, TxParams, WalletHandle,
};

/// Comma-separated trusted origins for the provider bridge.
///
/// Example:
/// `VAUGHAN_PROVIDER_TRUSTED_ORIGINS="https://wallet.freedom.local,https://app.freedom.local"`
const TRUSTED_ORIGINS_ENV: &str = "VAUGHAN_PROVIDER_TRUSTED_ORIGINS";

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
///
/// **Fail-loud (FR-2.4):** the bridge only starts when a trusted-origin
/// allowlist is configured via [`TRUSTED_ORIGINS_ENV`]. Without it (or with an
/// invalid one), the bridge does **not** start — there is no permissive
/// "accept any loopback client" mode. The wallet itself is unaffected.
pub fn spawn_provider_server(
    handle: &Handle,
    requests: mpsc::UnboundedSender<HostRequest>,
    events: EventBus,
) {
    handle.spawn(async move {
        let trusted_origins = match bridge_decision(env::var(TRUSTED_ORIGINS_ENV).ok().as_deref()) {
            Some(origins) => origins,
            None => {
                tracing::warn!(
                    env = TRUSTED_ORIGINS_ENV,
                    "provider trusted-origin allowlist not configured (or invalid); dApp bridge disabled"
                );
                return;
            }
        };
        let server = match ProviderServer::bind(DEFAULT_PORT).await {
            Ok(server) => server,
            Err(e) => {
                tracing::warn!(error = %e, "provider server failed to bind; dApp bridge disabled");
                return;
            }
        };
        let server = match configure_server_trusted_origins(server, &trusted_origins) {
            Ok(server) => server,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    env = TRUSTED_ORIGINS_ENV,
                    "provider trusted-origin config invalid; bridge disabled"
                );
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

/// The trusted-origin decision for the provider bridge (testable, pure).
///
/// `Some(origins)` = start the bridge with this allowlist. `None` = do not
/// start (env unset, empty, or invalid — all fail loud).
fn bridge_decision(raw_env: Option<&str>) -> Option<Vec<String>> {
    let origins = parse_trusted_origins(raw_env);
    if origins.is_empty() {
        return None;
    }
    // Validate now so a typo disables the bridge loudly instead of crashing at
    // serve time; `with_trusted_origins` is what actually canonicalizes.
    if let Err(e) = validate_origins(&origins) {
        tracing::warn!(error = %e, "provider trusted-origin config invalid");
        return None;
    }
    Some(origins)
}

/// Canonicalize + validate origins (mirrors `TrustedHosts::try_from_origins`
/// in vaughan-provider) so `bridge_decision` can reject bad input up front.
fn validate_origins(origins: &[String]) -> Result<(), ProviderError> {
    for origin in origins {
        let url = url::Url::parse(origin)
            .map_err(|_| ProviderError::InvalidParams(format!("invalid trusted origin `{origin}`")))?;
        if matches!(url.origin(), url::Origin::Opaque(_)) {
            return Err(ProviderError::InvalidParams(format!(
                "invalid trusted origin `{origin}` (origin must include host)"
            )));
        }
    }
    Ok(())
}

fn parse_trusted_origins(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn configure_server_trusted_origins(
    server: ProviderServer,
    trusted_origins: &[String],
) -> Result<ProviderServer, ProviderError> {
    if trusted_origins.is_empty() {
        return Ok(server);
    }
    server.with_trusted_origins(trusted_origins.iter().map(String::as_str))
}

/// Render a short, human-readable summary of `kind` for the approval prompt.
///
/// Returns `(title, details)` where `details` is shown verbatim. This is the
/// "show the user the full request" step of the signing guardrail; no secret
/// material is involved (these are the request fields the dApp sent).
pub fn describe_approval(
    kind: &ApprovalKind,
    wallet: &WalletState,
    handle: &Handle,
) -> Result<(String, Vec<String>), ProviderError> {
    match kind {
        ApprovalKind::SendTransaction(tx) => Ok((
            "Sign & broadcast transaction".into(),
            describe_tx(tx, wallet, tx_fee_for_prompt(tx, wallet, handle)?),
        )),
        ApprovalKind::SignTransaction(tx) => Ok((
            "Sign transaction (no broadcast)".into(),
            describe_tx(tx, wallet, tx_fee_for_prompt(tx, wallet, handle)?),
        )),
        ApprovalKind::SignMessage { message, .. } => Ok((
            "Sign message (personal_sign)".into(),
            vec![
                "Method:  personal_sign".to_string(),
                format!("Message: {message}"),
            ],
        )),
        ApprovalKind::SignTypedData { typed_data, .. } => {
            let primary = typed_data["primaryType"].as_str().unwrap_or("?");
            let domain = typed_data["domain"]["name"].as_str().unwrap_or("?");
            Ok((
                "Sign typed data (eth_signTypedData_v4)".into(),
                vec![
                    "Method:  eth_signTypedData_v4".to_string(),
                    format!("Domain:  {domain}"),
                    format!("Type:    {primary}"),
                ],
            ))
        }
    }
}

/// Describe a transaction's recipient, value, chain, data, and fee.
fn describe_tx(tx: &TxParams, wallet: &WalletState, fee: Fee) -> Vec<String> {
    let net = wallet.networks().active();
    let to = tx.to.as_deref().unwrap_or("(contract create)");
    let value = format_base_units(tx.value.as_deref().unwrap_or("0"), net.decimals);
    let testnet = if net.is_testnet { " (testnet)" } else { "" };
    let mut lines = vec![
        format!("To:      {to}"),
        format!("Value:   {value} {}", net.native_symbol),
        format!("Network: {}{testnet}", net.name),
        format!("Fee:     {}", fee.total),
    ];
    if let Some(data) = tx.data.as_deref() {
        lines.push(format!("Data:    {data}"));
    }
    lines
}

/// Compute the fee shown in the approval prompt.
///
/// Uses explicit tx gas fields when present (no network call); otherwise
/// estimates against the active network before prompting.
fn tx_fee_for_prompt(
    tx: &TxParams,
    wallet: &WalletState,
    handle: &Handle,
) -> Result<Fee, ProviderError> {
    if let Some(fee) = fee_from_explicit_tx_params(tx, wallet) {
        return Ok(fee);
    }
    let evm = to_evm_transaction(tx, wallet)?;
    handle
        .block_on(wallet.estimate_transaction_fee(evm))
        .map_err(map_wallet_error)
}

fn fee_from_explicit_tx_params(tx: &TxParams, wallet: &WalletState) -> Option<Fee> {
    let gas = tx.gas.as_deref()?.parse::<u128>().ok()?;
    let price = tx
        .max_fee_per_gas
        .as_deref()
        .or(tx.gas_price.as_deref())?
        .parse::<u128>()
        .ok()?;
    let total_wei = gas.checked_mul(price)?;
    let net = wallet.networks().active();
    let gas_limit = u64::try_from(gas).ok()?;
    Some(Fee {
        total: format!(
            "{} {}",
            format_base_units(&total_wei.to_string(), net.decimals),
            net.native_symbol
        ),
        currency: net.native_symbol.to_string(),
        details: vaughan_core::chains::FeeDetails::Evm {
            gas_limit,
            max_fee_per_gas: tx.max_fee_per_gas.clone().or(tx.gas_price.clone()),
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas.clone(),
        },
    })
}

/// Execute an approved request, producing the value the handler returns.
///
/// Async: the two network-backed paths (sign-only raw tx and broadcast) await
/// the wallet directly, so both the TUI (via the sync wrapper) and async
/// callers (integration tests) can drive the same code.
pub async fn execute_approval(
    kind: &ApprovalKind,
    wallet: &WalletState,
) -> Result<String, ProviderError> {
    // A locked wallet never prompts, never signs: reject cleanly (EIP-1193
    // 4100). The UI skips the prompt for locked wallets too (see app.rs), so
    // this guard is the shared backstop for every caller.
    if !wallet.is_unlocked() {
        return Err(ProviderError::Unauthorized(
            "wallet is locked; unlock it first".to_string(),
        ));
    }
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
            wallet.sign_transaction(evm).await.map_err(map_wallet_error)
        }
        ApprovalKind::SendTransaction(tx) => {
            let evm = to_evm_transaction(tx, wallet)?;
            wallet
                .send_transaction(evm)
                .await
                .map(|hash| hash.to_string())
                .map_err(map_wallet_error)
        }
    }
}

/// Synchronous wrapper for the UI thread: runs [`execute_approval`] via
/// `handle.block_on`. The UI thread is not a tokio worker, so blocking here is
/// safe (matching the existing view pattern).
pub fn execute_approval_sync(
    kind: &ApprovalKind,
    wallet: &WalletState,
    handle: &Handle,
) -> Result<String, ProviderError> {
    handle.block_on(execute_approval(kind, wallet))
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
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time::timeout;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::protocol::Message;
    use vaughan_core::chains::evm::networks::get_network_by_chain_id;
    use vaughan_provider::RequestHandler;

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
        let rt = tokio::runtime::Runtime::new().unwrap();
        let handle = rt.handle().clone();
        let (title, details) = describe_approval(&kind, &wallet, &handle).unwrap();
        assert_eq!(title, "Sign message (personal_sign)");
        assert!(details.iter().any(|l| l.contains("hello")));
    }

    #[test]
    fn describe_transaction_includes_fee_from_explicit_fields() {
        let path = tempfile::tempdir().unwrap().path().join("w.json");
        let mut wallet = WalletState::load(path).unwrap();
        let mnemonic = vaughan_core::security::hd_wallet::validate_mnemonic(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .unwrap();
        let password = secrecy::SecretString::from("CorrectHorse9!BatteryStaple".to_string());
        wallet.create(&password, mnemonic).unwrap();
        let kind = ApprovalKind::SignTransaction(TxParams {
            from: None,
            to: Some("0x0000000000000000000000000000000000000000".into()),
            data: Some("0x".into()),
            value: Some("1".into()),
            gas: Some("21000".into()),
            gas_price: Some("1000000000".into()),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: Some("0".into()),
            chain_id: None,
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        let handle = rt.handle().clone();
        let (_, details) = describe_approval(&kind, &wallet, &handle).unwrap();
        assert!(details.iter().any(|l| l.starts_with("Fee:")));
        assert!(details.iter().any(|l| l.contains("0.000021")));
    }

    #[test]
    fn chain_id_hex_roundtrip() {
        // Sanity check the chain-id formatting used by the UI (`0x{id:x}`).
        let net = get_network_by_chain_id(369).unwrap();
        assert_eq!(format!("0x{:x}", net.chain_id), "0x171");
    }

    #[test]
    fn parse_trusted_origins_handles_empty_and_csv() {
        assert!(parse_trusted_origins(None).is_empty());
        assert!(parse_trusted_origins(Some(" ,  ")).is_empty());
        assert_eq!(
            parse_trusted_origins(Some("https://a.example, https://b.example  ,")),
            vec![
                "https://a.example".to_string(),
                "https://b.example".to_string()
            ]
        );
    }

    #[test]
    fn bridge_decision_fails_loud_without_valid_allowlist() {
        // Unset / empty / whitespace-only env -> bridge does not start.
        assert!(bridge_decision(None).is_none());
        assert!(bridge_decision(Some("")).is_none());
        assert!(bridge_decision(Some(" ,  ")).is_none());
        // Invalid origin (no scheme) -> bridge does not start.
        assert!(bridge_decision(Some("not-an-origin")).is_none());
        // Valid allowlist -> bridge starts with exactly those origins.
        let origins = bridge_decision(Some("https://app.example, https://wallet.freedom.local"))
            .expect("valid allowlist starts the bridge");
        assert_eq!(
            origins,
            vec![
                "https://app.example".to_string(),
                "https://wallet.freedom.local".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn startup_allowlist_rejects_missing_origin_and_allows_trusted_origin() {
        struct OkHandler;
        #[async_trait]
        impl RequestHandler for OkHandler {
            async fn handle(
                &self,
                _ctx: RequestCtx,
                _request: vaughan_provider::RpcRequest,
            ) -> vaughan_provider::HandlerResult {
                Ok(json!("ok"))
            }
        }

        let server = ProviderServer::bind(0).await.unwrap();
        let trusted_origins = vec!["https://app.example".to_string()];
        let server = configure_server_trusted_origins(server, &trusted_origins).unwrap();
        let url = server.url();
        let events = EventBus::new();
        let task = tokio::spawn(server.serve(Arc::new(OkHandler), events));

        // Startup-configured allowlist blocks clients with no Origin header.
        let (mut ws, _) = connect_async(&url).await.unwrap();
        ws.send(Message::Text(
            r#"{"jsonrpc":"2.0","id":1,"method":"eth_chainId"}"#.into(),
        ))
        .await
        .unwrap();
        let close = timeout(Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .expect("socket closes");
        assert!(matches!(close, Ok(Message::Close(_)) | Err(_)));

        // The same startup-configured allowlist accepts the trusted origin.
        let mut request = url.clone().into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Origin", "https://app.example/".parse().unwrap());
        let (mut ws, _) = connect_async(request).await.unwrap();
        ws.send(Message::Text(
            r#"{"jsonrpc":"2.0","id":2,"method":"eth_chainId"}"#.into(),
        ))
        .await
        .unwrap();
        let reply = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Text(text) = reply else {
            panic!("expected text response")
        };
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["id"], 2);
        assert_eq!(value["result"], "ok");

        task.abort();
    }
}
