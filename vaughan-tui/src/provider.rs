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
use std::sync::{Arc, Mutex};

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use async_trait::async_trait;
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};

use vaughan_core::chains::{EvmTransaction, Fee};
use vaughan_core::core::proposal::{apply_proposal, ProposalQueue, TxProposal};
use vaughan_core::core::{format_base_units, WalletState};
use vaughan_core::error::WalletError;
use vaughan_provider::server::DEFAULT_PORT;
use vaughan_provider::{
    Eip1193Handler, EventBus, ProviderError, ProviderServer, RequestCtx, TxParams, WalletHandle,
};

/// Comma-separated trusted origins for the provider bridge (additive).
///
/// Freedom's Vaughan signer always sends [`FREEDOM_PROVIDER_ORIGIN`]; that
/// origin is merged automatically. Extra env / dApp origins remain optional.
///
/// Example:
/// `VAUGHAN_PROVIDER_TRUSTED_ORIGINS="https://app.example"`
const TRUSTED_ORIGINS_ENV: &str = "VAUGHAN_PROVIDER_TRUSTED_ORIGINS";

/// Origin Freedom Browser's Vaughan WS transport sends on every connect.
///
/// Must stay in sync with Freedom's `DEFAULT_ORIGIN` (`https://freedom.browser`).
pub const FREEDOM_PROVIDER_ORIGIN: &str = "https://freedom.browser";

/// Origin of the Vaughan dApp-browser unpacked extension (stable manifest `key`).
///
/// The extension owns the WebSocket (CSP-safe). Handshake Origin is this
/// chrome-extension URL, not the page host — required for PulseX IPFS gateways
/// and sites that block `ws://` in `connect-src` (e.g. 9inch).
pub const DAPP_BROWSER_PROVIDER_ORIGIN: &str =
    "chrome-extension://cneeaoilhnioopaiaidjadinahpgacpn";

/// Snapshot of the local EIP-1193 bridge for the Web (Freedom) screen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BridgeStatus {
    /// Spawn task has not reported yet.
    #[default]
    Starting,
    /// Listening on loopback (URL is `ws://127.0.0.1:<port>`).
    Listening { url: String },
    /// Bridge did not start (bind failure, invalid config, etc.).
    Disabled { reason: String },
}

impl BridgeStatus {
    /// One-line status for the Web screen.
    pub fn summary_line(&self) -> String {
        match self {
            Self::Starting => "Bridge: starting…".into(),
            Self::Listening { url } => {
                format!(
                    "Bridge: {url} (unlock → open dApp; green banner = inject; approve sign/send here)"
                )
            }
            Self::Disabled { reason } => format!("Bridge: disabled — {reason}"),
        }
    }
}

/// Shared bridge status updated by [`spawn_provider_server`].
pub type BridgeStatusHandle = Arc<Mutex<BridgeStatus>>;

/// Create a shared [`BridgeStatus`] handle (starts as [`BridgeStatus::Starting`]).
pub fn new_bridge_status() -> BridgeStatusHandle {
    Arc::new(Mutex::new(BridgeStatus::Starting))
}

fn set_bridge_status(status: &BridgeStatusHandle, next: BridgeStatus) {
    if let Ok(mut guard) = status.lock() {
        *guard = next;
    }
}

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
    /// Connect gesture (`eth_requestAccounts`) for a site key.
    Connect { site: String },
    /// Switch the active Vaughan network.
    SwitchChain { chain_id: String, label: String },
    /// MCP / external agent transaction proposal.
    McpProposal {
        proposal_id: String,
        source: String,
        proposal: Box<TxProposal>,
    },
    /// Sweep an ERC-5564 stealth note back to the active account.
    StealthSweep {
        stealth_address: String,
        balance_display: String,
    },
}

/// A request forwarded from the provider handler to the UI thread.
pub enum HostRequest {
    /// `eth_accounts`: accounts only if this site already connected this session.
    Accounts {
        site: String,
        reply: oneshot::Sender<Result<Vec<String>, ProviderError>>,
    },
    /// `eth_requestAccounts`: connect gesture (may prompt).
    RequestAccounts {
        site: String,
        reply: oneshot::Sender<Result<Vec<String>, ProviderError>>,
    },
    /// `eth_chainId`: the active chain id as `0x` hex.
    ChainId {
        reply: oneshot::Sender<Result<String, ProviderError>>,
    },
    /// `wallet_switchEthereumChain`: switch to a built-in network (prompts).
    SwitchChain {
        chain_id: String,
        /// Best-effort display origin (page origin preferred) for the prompt.
        origin: Option<String>,
        reply: oneshot::Sender<Result<(), ProviderError>>,
    },
    /// A sign/send request needing user approval.
    Approval {
        kind: Box<ApprovalKind>,
        /// Best-effort display origin (page origin preferred).
        origin: Option<String>,
        /// Site key for connect-grant checks (page origin → WS origin → peer).
        site: String,
        /// True for extension-originated requests, which must hold a Connect
        /// grant before they may prompt for sign/send. Freedom's transport is
        /// exempt: its own browser chrome is the connect gesture.
        requires_grant: bool,
        reply: oneshot::Sender<Result<String, ProviderError>>,
    },
}

/// The [`WalletHandle`] handed to the provider server. Cheaply cloneable via
/// the shared request channel; the UI thread is the single consumer.
pub struct ProviderHost {
    requests: mpsc::Sender<HostRequest>,
}

impl ProviderHost {
    pub fn new(requests: mpsc::Sender<HostRequest>) -> Self {
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
                origin: display_origin(ctx),
                site: site_key(ctx),
                requires_grant: is_extension_path(ctx),
                reply,
            })
            .await
            .map_err(Self::disconnected)?;
        rx.await.map_err(Self::dropped)?
    }
}

/// Prefer page origin (extension) over WebSocket Origin for UI / grants.
pub fn display_origin(ctx: &RequestCtx) -> Option<String> {
    ctx.page_origin.clone().or_else(|| ctx.origin.clone())
}

/// Stable site key for connect grants (page origin → WS origin → peer).
pub fn site_key(ctx: &RequestCtx) -> String {
    display_origin(ctx).unwrap_or_else(|| format!("peer:{}", ctx.peer))
}

/// Whether this request came in over the dApp-browser extension path (its WS
/// Origin is the extension id, and it may carry an attested page origin).
/// Extension-path requests must hold a Connect grant before prompting to sign.
pub fn is_extension_path(ctx: &RequestCtx) -> bool {
    ctx.page_origin.is_some() || ctx.origin.as_deref() == Some(DAPP_BROWSER_PROVIDER_ORIGIN)
}

#[async_trait]
impl WalletHandle for ProviderHost {
    async fn accounts(&self, ctx: &RequestCtx) -> Result<Vec<String>, ProviderError> {
        let (reply, rx) = oneshot::channel();
        self.requests
            .send(HostRequest::Accounts {
                site: site_key(ctx),
                reply,
            })
            .await
            .map_err(Self::disconnected)?;
        rx.await.map_err(Self::dropped)?
    }

    async fn request_accounts(&self, ctx: &RequestCtx) -> Result<Vec<String>, ProviderError> {
        let (reply, rx) = oneshot::channel();
        self.requests
            .send(HostRequest::RequestAccounts {
                site: site_key(ctx),
                reply,
            })
            .await
            .map_err(Self::disconnected)?;
        rx.await.map_err(Self::dropped)?
    }

    async fn chain_id(&self, _ctx: &RequestCtx) -> Result<String, ProviderError> {
        let (reply, rx) = oneshot::channel();
        self.requests
            .send(HostRequest::ChainId { reply })
            .await
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

    async fn switch_chain(&self, ctx: &RequestCtx, chain_id: &str) -> Result<(), ProviderError> {
        let (reply, rx) = oneshot::channel();
        self.requests
            .send(HostRequest::SwitchChain {
                chain_id: chain_id.to_string(),
                origin: display_origin(ctx),
                reply,
            })
            .await
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
/// **Fail-loud (FR-2.4):** the bridge always includes [`FREEDOM_PROVIDER_ORIGIN`]
/// and [`DAPP_BROWSER_PROVIDER_ORIGIN`] so Freedom and the Vaughan Chromium
/// extension can connect without an env ritual. Env vars and persisted dApp
/// origins are merged on top. Invalid origins disable the bridge (no permissive
/// "accept any loopback client" mode). The wallet itself is unaffected when the
/// bridge is down.
pub fn spawn_provider_server(
    handle: &Handle,
    requests: mpsc::Sender<HostRequest>,
    events: EventBus,
    extra_origins: Vec<String>,
    status: BridgeStatusHandle,
    profile_dir: std::path::PathBuf,
    grants: std::sync::Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
) {
    handle.spawn(async move {
        let trusted_origins = match bridge_decision(
            env::var(TRUSTED_ORIGINS_ENV).ok().as_deref(),
            &extra_origins,
        ) {
            Some(origins) => origins,
            None => {
                let reason = "trusted-origin allowlist invalid".to_string();
                tracing::warn!(
                    env = TRUSTED_ORIGINS_ENV,
                    "provider trusted-origin allowlist invalid; dApp bridge disabled"
                );
                set_bridge_status(&status, BridgeStatus::Disabled { reason });
                return;
            }
        };
        let session = vaughan_core::core::ProviderSessionToken::generate();
        if let Err(e) = session.write(&profile_dir) {
            tracing::warn!(error = %e, "provider session token write failed; bridge disabled");
            set_bridge_status(
                &status,
                BridgeStatus::Disabled {
                    reason: format!("session token write failed ({e})"),
                },
            );
            return;
        }
        // Dev/test override for the listen port (parallel instances); the
        // packaged default stays DEFAULT_PORT.
        let port = env::var("VAUGHAN_PROVIDER_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);
        let server = match ProviderServer::bind(port).await {
            Ok(server) => server,
            Err(e) => {
                let reason = format!("bind failed ({e})");
                tracing::warn!(error = %e, "provider server failed to bind; dApp bridge disabled");
                set_bridge_status(&status, BridgeStatus::Disabled { reason });
                return;
            }
        };
        let server = match configure_server_trusted_origins(server, &trusted_origins) {
            Ok(server) => server
                .with_session_token(session.as_str())
                // Only the attested extension may speak for page origins.
                .with_page_origin_issuers([DAPP_BROWSER_PROVIDER_ORIGIN])
                // Live grant set for per-connection accountsChanged filtering.
                .with_grants(grants),
            Err(e) => {
                let reason = format!("origin config invalid ({e})");
                tracing::warn!(
                    error = %e,
                    env = TRUSTED_ORIGINS_ENV,
                    "provider trusted-origin config invalid; bridge disabled"
                );
                set_bridge_status(&status, BridgeStatus::Disabled { reason });
                return;
            }
        };
        let url = server.url();
        tracing::info!(%url, "provider server listening (session token required for all origins)");
        set_bridge_status(&status, BridgeStatus::Listening { url });
        let handler = Arc::new(Eip1193Handler::new(Arc::new(ProviderHost::new(requests))));
        if let Err(e) = server.serve(handler, events).await {
            tracing::warn!(error = %e, "provider server stopped");
            let _ = vaughan_core::core::ProviderSessionToken::invalidate(&profile_dir);
            set_bridge_status(
                &status,
                BridgeStatus::Disabled {
                    reason: format!("server stopped ({e})"),
                },
            );
        }
    });
}

/// The trusted-origin decision for the provider bridge (testable, pure).
///
/// Always includes [`FREEDOM_PROVIDER_ORIGIN`] and
/// [`DAPP_BROWSER_PROVIDER_ORIGIN`]. Merges env CSV + `extra_origins`
/// (persisted dApp whitelist). `None` only when validation fails (e.g. a bad
/// env entry) — an empty env/extras list still starts the bridge.
fn bridge_decision(raw_env: Option<&str>, extra_origins: &[String]) -> Option<Vec<String>> {
    let mut origins = vec![
        FREEDOM_PROVIDER_ORIGIN.to_string(),
        DAPP_BROWSER_PROVIDER_ORIGIN.to_string(),
    ];
    for o in parse_trusted_origins(raw_env)
        .into_iter()
        .chain(extra_origins.iter().cloned())
    {
        let o = o.trim();
        if o.is_empty() {
            continue;
        }
        if !origins.iter().any(|e| e.eq_ignore_ascii_case(o)) {
            origins.push(o.to_string());
        }
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
        let url = url::Url::parse(origin).map_err(|_| {
            ProviderError::InvalidParams(format!("invalid trusted origin `{origin}`"))
        })?;
        if let url::Origin::Opaque(_) = url.origin() {
            if url.scheme() == "chrome-extension" && url.host_str().is_some() {
                continue;
            }
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

/// Pretty-print a signed payload for the approval prompt, indented and capped
/// so a huge blob cannot flood the terminal (the full bytes are still what
/// gets signed — this is display-only).
fn pretty_payload_lines(value: &Value) -> Vec<String> {
    const MAX_LINES: usize = 60;
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    let mut lines: Vec<String> = pretty.lines().map(|l| format!("  {l}")).collect();
    if lines.len() > MAX_LINES {
        let omitted = lines.len() - MAX_LINES;
        lines.truncate(MAX_LINES);
        lines.push(format!("  … ({omitted} more lines)"));
    }
    lines
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
    let (title, details, _) = describe_approval_with_fee(kind, wallet, handle)?;
    Ok((title, details))
}

/// [`describe_approval`] plus the base [`Fee`] for transaction kinds, so the
/// approve view can offer speed presets / custom gas before signing.
pub fn describe_approval_with_fee(
    kind: &ApprovalKind,
    wallet: &WalletState,
    handle: &Handle,
) -> Result<(String, Vec<String>, Option<Fee>), ProviderError> {
    match kind {
        ApprovalKind::SendTransaction(tx) => {
            let fee = tx_fee_for_prompt(tx, wallet, handle)?;
            Ok((
                "Sign & broadcast transaction".into(),
                describe_tx(tx, wallet, fee.clone()),
                Some(fee),
            ))
        }
        ApprovalKind::SignTransaction(tx) => {
            let fee = tx_fee_for_prompt(tx, wallet, handle)?;
            Ok((
                "Sign transaction (no broadcast)".into(),
                describe_tx(tx, wallet, fee.clone()),
                Some(fee),
            ))
        }
        ApprovalKind::SignMessage { message, .. } => Ok((
            "Sign message (personal_sign)".into(),
            vec![
                "Method:  personal_sign".to_string(),
                format!("Message: {message}"),
            ],
            None,
        )),
        ApprovalKind::SignTypedData { typed_data, .. } => {
            let primary = typed_data["primaryType"].as_str().unwrap_or("?");
            let domain = &typed_data["domain"];
            let domain_name = domain["name"].as_str().unwrap_or("?");
            let mut lines = vec![
                "Method:  eth_signTypedData_v4".to_string(),
                format!("Domain:  {domain_name}"),
            ];
            // chainId + verifyingContract are what a phishing signature abuses
            // (Permit2-style drains) — always show them when present.
            let chain = domain["chainId"]
                .as_u64()
                .or_else(|| domain["chainId"].as_str()?.parse::<u64>().ok());
            if let Some(chain) = chain {
                lines.push(format!("Chain:   {chain}"));
            }
            if let Some(contract) = domain["verifyingContract"].as_str() {
                lines.push(format!("Contract:{contract}"));
            }
            lines.push(format!("Type:    {primary}"));
            lines.push("Message:".to_string());
            lines.extend(pretty_payload_lines(&typed_data["message"]));
            Ok(("Sign typed data (eth_signTypedData_v4)".into(), lines, None))
        }
        ApprovalKind::Connect { site } => Ok((
            "Connect dApp (eth_requestAccounts)".into(),
            vec![
                format!("Site:    {site}"),
                "Grants this site your active account until you lock the wallet.".into(),
                "Sign/send still requires a separate approval.".into(),
            ],
            None,
        )),
        ApprovalKind::SwitchChain { chain_id, label } => Ok((
            "Switch network".into(),
            vec![format!("Chain:   {label}"), format!("Id:      {chain_id}")],
            None,
        )),
        ApprovalKind::McpProposal {
            proposal, source, ..
        } => {
            let net = wallet.networks().active();
            let to = format!("{:#x}", proposal.to);
            let value = format_base_units(&proposal.value_wei.to_string(), net.decimals);
            let testnet = if net.is_testnet { " (testnet)" } else { "" };
            let data = if proposal.calldata.is_empty() {
                None
            } else {
                Some(format!("0x{}", hex::encode(&proposal.calldata)))
            };
            let mut lines = vec![
                format!("Source:  MCP ({source})"),
                format!("To:      {to}"),
                format!("Value:   {value} {}", net.native_symbol),
                format!("Network: {}{testnet}", net.name),
                format!("Gas:     {}", proposal.gas_limit),
                mcp_proposal_fee_line(wallet, proposal, handle),
                format!(
                    "Sim (agent): {}",
                    if proposal.simulation_success {
                        "ok"
                    } else {
                        "failed — will re-simulate"
                    }
                ),
                "Note:    Agent explanation is UNTRUSTED — verify calldata below".to_string(),
                format!("Agent:   {}", proposal.explanation),
            ];
            if let Some(d) = data {
                lines.push(format!("Data:    {d}"));
            }
            Ok(("MCP transaction proposal".into(), lines, None))
        }
        ApprovalKind::StealthSweep {
            stealth_address,
            balance_display,
        } => {
            let net = wallet.networks().active();
            let testnet = if net.is_testnet { " (testnet)" } else { "" };
            Ok((
                "Sweep stealth note".into(),
                vec![
                    format!("From:    {stealth_address}"),
                    format!("Amount:  {balance_display}"),
                    format!("Network: {}{testnet}", net.name),
                    "Moves funds to your active public account.".into(),
                ],
                None,
            ))
        }
    }
}

/// Fee line for the MCP proposal card (guardrail: the prompt must show cost).
///
/// Prefers a fresh estimate built from the proposal itself; falls back to the
/// agent-stamped estimate (labeled unverified), then to "unavailable".
/// Batch7702 drafts cannot be fee-estimated without a signature, so they show
/// the agent-stamped Ambire self-pay estimate directly.
fn mcp_proposal_fee_line(wallet: &WalletState, proposal: &TxProposal, handle: &Handle) -> String {
    let net = wallet.networks().active();
    let agent_est = proposal
        .estimated_fee_wei
        .filter(|v| !v.is_zero())
        .map(|wei| {
            format!(
                "~{} {} (agent estimate, unverified)",
                format_base_units(&wei.to_string(), net.decimals),
                net.native_symbol
            )
        });
    let is_batch = matches!(
        proposal.proposal_type,
        vaughan_core::core::proposal::ProposalType::Batch7702 { .. }
    );
    let fresh = if is_batch {
        None
    } else {
        apply_proposal(wallet, proposal)
            .ok()
            .and_then(|evm| handle.block_on(wallet.estimate_transaction_fee(evm)).ok())
    };
    match (fresh, agent_est) {
        (Some(fee), _) => format!("Fee:     {}", fee.total),
        (None, Some(est)) => format!("Fee:     {est}"),
        (None, None) => "Fee:     unavailable".to_string(),
    }
}

/// Apply a user-adjusted prompt fee to a transaction's gas fields.
///
/// The fee type follows the transaction's original shape: legacy `gasPrice`
/// transactions keep a legacy price (adjusted max becomes the price), and
/// EIP-1559 transactions get the adjusted max/tip pair. Gas limit is pinned
/// to the prompt's value so signing does not silently re-estimate a
/// different number than the user approved.
pub fn apply_fee_override(tx: &mut TxParams, fee: &Fee) {
    let vaughan_core::chains::FeeDetails::Evm {
        gas_limit,
        max_fee_per_gas,
        max_priority_fee_per_gas,
    } = &fee.details
    else {
        return;
    };
    tx.gas = Some(gas_limit.to_string());
    if tx.gas_price.is_some() {
        // Legacy-shaped tx: the adjusted max becomes the legacy gas price.
        tx.gas_price = max_fee_per_gas.clone();
    } else {
        tx.max_fee_per_gas = max_fee_per_gas.clone();
        tx.max_priority_fee_per_gas = max_priority_fee_per_gas.clone();
        tx.gas_price = None;
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
            if let Some(from) = tx.from.as_deref() {
                verify_address(from, wallet)?;
            }
            let evm = to_evm_transaction(tx, wallet)?;
            wallet.sign_transaction(evm).await.map_err(map_wallet_error)
        }
        ApprovalKind::SendTransaction(tx) => {
            if let Some(from) = tx.from.as_deref() {
                verify_address(from, wallet)?;
            }
            let evm = to_evm_transaction(tx, wallet)?;
            wallet
                .send_transaction(evm)
                .await
                .map(|hash| hash.to_string())
                .map_err(map_wallet_error)
        }
        ApprovalKind::Connect { .. } | ApprovalKind::SwitchChain { .. } => {
            // Handled in `App::handle_approval_key` (grants / network switch).
            Ok("ok".into())
        }
        ApprovalKind::McpProposal {
            proposal_id,
            proposal,
            ..
        } => {
            use vaughan_core::core::proposal::ProposalType;
            if matches!(proposal.proposal_type, ProposalType::Batch7702 { .. }) {
                // Ambire self-pay path: dummy draft signature cannot eth_call.
                // Integrity check = abi-decode execute(txns) then submit_batch (fresh sig).
                // Fee spike uses the same pinned-gas estimate as submit_batch.
                let txns = vaughan_aa::decode_execute(&proposal.calldata)
                    .map_err(ProviderError::Internal)?;
                if txns.is_empty() {
                    return Err(ProviderError::Internal(
                        "batch7702: decoded execute had zero calls".into(),
                    ));
                }
                let signer = wallet.active_signer().map_err(map_wallet_error)?;
                let adapter = wallet.active_adapter().await.map_err(map_wallet_error)?;
                let account = signer.address();
                let chain_id = wallet.networks().active().chain_id;
                let nonce = vaughan_aa::get_account_nonce(&adapter, account)
                    .await
                    .unwrap_or(0);
                let scw = vaughan_aa::ScwTransaction {
                    account,
                    chain_id,
                    nonce,
                    txns: txns.clone(),
                };
                let placeholder = [0u8; 66];
                if let Ok((gas_limit, max_fee, _)) =
                    vaughan_aa::estimate_self_pay_fee(&adapter, &scw, &placeholder, None).await
                {
                    let fresh_wei = U256::from(gas_limit).saturating_mul(U256::from(max_fee));
                    if vaughan_core::core::fee_spike_exceeds_threshold(
                        proposal.estimated_fee_wei,
                        fresh_wei,
                    ) {
                        return Err(ProviderError::InvalidParams(
                            "network fee is unverified or increased more than 10% since \
                             the agent proposal — deny and ask the agent to re-propose"
                                .into(),
                        ));
                    }
                }
                let result = vaughan_aa::submit_batch(
                    &adapter,
                    &signer,
                    txns,
                    vaughan_aa::AMBIRE_IMPLEMENTATION,
                )
                .await
                .map_err(map_wallet_error)?;
                if let Some(parent) = wallet.path().parent() {
                    if let Ok(Some(token)) = vaughan_core::core::McpSessionToken::read(parent) {
                        let queue = ProposalQueue::new(parent);
                        let _ = queue.mark_approved(
                            proposal_id,
                            &result.tx_hash.to_string(),
                            token.as_bytes(),
                        );
                    }
                }
                return Ok(result.tx_hash.to_string());
            }
            resimulate_mcp_proposal(wallet, proposal).await?;
            let evm = apply_proposal(wallet, proposal).map_err(map_wallet_error)?;
            if let Ok(fresh_fee) = wallet.estimate_transaction_fee(evm.clone()).await {
                if let Some(fresh_wei) = fresh_fee.total_wei_evm() {
                    if vaughan_core::core::fee_spike_exceeds_threshold(
                        proposal.estimated_fee_wei,
                        fresh_wei,
                    ) {
                        return Err(ProviderError::InvalidParams(
                            "network fee is unverified or increased more than 10% since \
                             the agent proposal — deny and ask the agent to re-propose"
                                .into(),
                        ));
                    }
                }
            }
            let hash = wallet
                .send_transaction(evm)
                .await
                .map_err(map_wallet_error)?;
            if let Some(parent) = wallet.path().parent() {
                if let Ok(Some(token)) = vaughan_core::core::McpSessionToken::read(parent) {
                    let queue = ProposalQueue::new(parent);
                    let _ = queue.mark_approved(proposal_id, &hash.to_string(), token.as_bytes());
                }
            }
            Ok(hash.to_string())
        }
        ApprovalKind::StealthSweep {
            stealth_address, ..
        } => {
            let notes = wallet
                .scan_stealth_notes()
                .await
                .map_err(map_wallet_error)?;
            let note = notes
                .into_iter()
                .find(|n| {
                    format!("{:#x}", n.announcement.stealth_address)
                        .eq_ignore_ascii_case(stealth_address)
                })
                .ok_or_else(|| {
                    ProviderError::Internal(format!(
                        "no unswept stealth note for {stealth_address}"
                    ))
                })?;
            wallet
                .sweep_stealth_note(&note)
                .await
                .map(|h| h.to_string())
                .map_err(map_wallet_error)
        }
    }
}

/// FR-6.4: re-run `eth_call` at approve time so stale agent simulations cannot
/// bypass a fresh on-chain check immediately before sign/broadcast.
async fn resimulate_mcp_proposal(
    wallet: &WalletState,
    proposal: &TxProposal,
) -> Result<(), ProviderError> {
    let from_str = wallet.active_address().map_err(map_wallet_error)?;
    let from: Address = from_str
        .parse()
        .map_err(|_| ProviderError::Internal("active account address is invalid".into()))?;
    let adapter = wallet.active_adapter().await.map_err(map_wallet_error)?;
    let to = proposal.to;
    let value = proposal.value_wei;
    let data = proposal.calldata.clone();
    adapter
        .with_provider(|provider| {
            let data = data.clone();
            async move {
                let tx = alloy::rpc::types::eth::TransactionRequest::default()
                    .from(from)
                    .to(to)
                    .input(data.into())
                    .value(value);
                provider
                    .call(tx)
                    .await
                    .map(|_| ())
                    .map_err(|e| WalletError::RpcError(e.to_string()))
            }
        })
        .await
        .map_err(|e| match e {
            WalletError::RpcError(msg) => {
                ProviderError::InvalidParams(format!("simulation reverted: {msg}"))
            }
            other => map_wallet_error(other),
        })
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

    fn tx_params_with_gas(
        gas_price: Option<&str>,
        max_fee: Option<&str>,
        tip: Option<&str>,
    ) -> TxParams {
        TxParams {
            from: None,
            to: Some("0xabc".into()),
            data: None,
            value: None,
            gas: Some("21000".into()),
            gas_price: gas_price.map(Into::into),
            max_fee_per_gas: max_fee.map(Into::into),
            max_priority_fee_per_gas: tip.map(Into::into),
            nonce: None,
            chain_id: None,
        }
    }

    fn evm_fee(gas_limit: u64, max_fee: &str, tip: Option<&str>) -> Fee {
        Fee {
            total: "x".into(),
            currency: "tPLS".into(),
            details: vaughan_core::chains::FeeDetails::Evm {
                gas_limit,
                max_fee_per_gas: Some(max_fee.into()),
                max_priority_fee_per_gas: tip.map(Into::into),
            },
        }
    }

    #[test]
    fn apply_fee_override_preserves_legacy_shape() {
        let mut tx = tx_params_with_gas(Some("100"), None, None);
        apply_fee_override(&mut tx, &evm_fee(30_000, "250", Some("5")));
        // Legacy tx keeps a legacy price; the adjusted max becomes the price.
        assert_eq!(tx.gas_price.as_deref(), Some("250"));
        assert_eq!(tx.gas.as_deref(), Some("30000"));
        assert!(tx.max_fee_per_gas.is_none());
        assert!(tx.max_priority_fee_per_gas.is_none());
    }

    #[test]
    fn apply_fee_override_sets_eip1559_fields() {
        let mut tx = tx_params_with_gas(None, Some("100"), Some("2"));
        apply_fee_override(&mut tx, &evm_fee(30_000, "250", Some("5")));
        assert_eq!(tx.max_fee_per_gas.as_deref(), Some("250"));
        assert_eq!(tx.max_priority_fee_per_gas.as_deref(), Some("5"));
        assert_eq!(tx.gas.as_deref(), Some("30000"));
        assert!(tx.gas_price.is_none());
    }

    #[test]
    fn apply_fee_override_ignores_non_evm_fee() {
        let mut tx = tx_params_with_gas(Some("100"), None, None);
        let fee = Fee {
            total: "x".into(),
            currency: "BTC".into(),
            details: vaughan_core::chains::FeeDetails::Bitcoin {
                fee_rate_sat_per_vbyte: "1".into(),
                estimated_vsize: 250,
            },
        };
        apply_fee_override(&mut tx, &fee);
        assert_eq!(tx.gas_price.as_deref(), Some("100"));
        assert_eq!(tx.gas.as_deref(), Some("21000"));
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

    fn mcp_transfer_kind(id: &str, estimated_fee_wei: Option<U256>) -> ApprovalKind {
        let recipient = alloy::primitives::address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
        let mut proposal = TxProposal::new(
            id,
            vaughan_core::core::proposal::ProposalType::NativeTransfer {
                to: recipient,
                amount_wei: U256::from(1u64),
            },
            recipient,
            U256::from(1u64),
            alloy::primitives::Bytes::new(),
            21_000,
            true,
            "test",
        );
        proposal.estimated_fee_wei = estimated_fee_wei;
        ApprovalKind::McpProposal {
            proposal_id: id.into(),
            source: "test".into(),
            proposal: Box::new(proposal),
        }
    }

    #[test]
    fn describe_mcp_proposal_shows_agent_fee_when_fresh_unavailable() {
        // Locked wallet: fresh estimation is unavailable, so the card falls
        // back to the agent-stamped estimate (labeled unverified).
        let wallet = WalletState::load(tempfile::tempdir().unwrap().path().join("w.json")).unwrap();
        let kind = mcp_transfer_kind("prop_fee", Some(U256::from(1_000_000_000_000_000u64)));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let handle = rt.handle().clone();
        let (_, details) = describe_approval(&kind, &wallet, &handle).unwrap();
        let fee = details
            .iter()
            .find(|l| l.starts_with("Fee:"))
            .expect("MCP card must show a fee line");
        assert!(fee.contains("agent estimate"));
    }

    #[test]
    fn describe_mcp_proposal_marks_missing_fee_estimate() {
        let wallet = WalletState::load(tempfile::tempdir().unwrap().path().join("w.json")).unwrap();
        let kind = mcp_transfer_kind("prop_no_fee", None);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let handle = rt.handle().clone();
        let (_, details) = describe_approval(&kind, &wallet, &handle).unwrap();
        assert!(details.iter().any(|l| l == "Fee:     unavailable"));
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
    fn describe_typed_data_shows_full_message_payload() {
        let wallet = WalletState::load(tempfile::tempdir().unwrap().path().join("w.json")).unwrap();
        let typed_data = serde_json::json!({
            "primaryType": "Permit",
            "domain": {
                "name": "Permit2",
                "chainId": 1,
                "verifyingContract": "0x000000000022D473030F116dDEE9F6B43aC78BA3"
            },
            "message": {
                "permitted": {"token": "0xA0b8…", "amount": "115792089237316195423570985008687907853269984665640564039457"},
                "spender": "0xE20223c3f0aB0B1e02E4E0e3d2e5A1c5",
                "deadline": 1893456000u64
            }
        });
        let kind = ApprovalKind::SignTypedData {
            address: "0xabc".into(),
            typed_data,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let handle = rt.handle().clone();
        let (_, details) = describe_approval(&kind, &wallet, &handle).unwrap();
        // Domain security fields and the full message payload are visible.
        assert!(details.iter().any(|l| l.contains("Chain:   1")));
        assert!(details
            .iter()
            .any(|l| l.contains("0x000000000022D473030F116dDEE9F6B43aC78BA3")));
        assert!(details.iter().any(|l| l.contains("\"spender\"")));
        assert!(details.iter().any(|l| l.contains("1893456000")));
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
    fn bridge_decision_always_includes_freedom_and_dapp_browser_origins() {
        let none: &[String] = &[];
        let base = vec![
            FREEDOM_PROVIDER_ORIGIN.to_string(),
            DAPP_BROWSER_PROVIDER_ORIGIN.to_string(),
        ];
        // Empty env + no dApps → Freedom + dApp-browser extension Origins.
        assert_eq!(bridge_decision(None, none).expect("bridge starts"), base);
        assert_eq!(bridge_decision(Some(""), none).unwrap(), base);
        assert_eq!(bridge_decision(Some(" ,  "), none).unwrap(), base);
        // Invalid extra origin (no scheme) → bridge does not start.
        assert!(bridge_decision(Some("not-an-origin"), none).is_none());
        // Valid env origins merge after the built-ins (no duplicate Freedom).
        let origins = bridge_decision(Some("https://app.example, https://freedom.browser"), none)
            .expect("valid allowlist starts the bridge");
        assert_eq!(
            origins,
            vec![
                FREEDOM_PROVIDER_ORIGIN.to_string(),
                DAPP_BROWSER_PROVIDER_ORIGIN.to_string(),
                "https://app.example".to_string(),
            ]
        );
        // Persisted dApp origins merge after the built-ins.
        let from_dapps = bridge_decision(None, &["https://app.pulsex.com".into()])
            .expect("dApp whitelist merges with built-ins");
        assert_eq!(
            from_dapps,
            vec![
                FREEDOM_PROVIDER_ORIGIN.to_string(),
                DAPP_BROWSER_PROVIDER_ORIGIN.to_string(),
                "https://app.pulsex.com".to_string()
            ]
        );
        assert!(DAPP_BROWSER_PROVIDER_ORIGIN.starts_with("chrome-extension://"));
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
