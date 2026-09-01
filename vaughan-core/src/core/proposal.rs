//! Transaction proposals for external agents (MCP) and unified human approval.
//!
//! Agents draft structured [`TxProposal`]s; the TUI verifies HMAC, re-simulates,
//! and shows ground-truth calldata before signing.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::chains::{ChainTransaction, EvmTransaction};
use crate::core::transaction::TransactionService;
use crate::core::WalletState;
use crate::error::WalletError;

/// Default proposal TTL (seconds).
pub const PROPOSAL_TTL_SECS: u64 = 600;

/// Maximum pending proposals per profile.
pub const MAX_PENDING_PROPOSALS: usize = 10;

/// Max length for [`TxProposal::proposal_id`] (filesystem-safe).
pub const MAX_PROPOSAL_ID_LEN: usize = 64;

/// Sliding window for MCP enqueue rate limiting (seconds).
pub const MCP_ENQUEUE_RATE_WINDOW_SECS: u64 = 60;

/// Max proposal enqueues per profile per [`MCP_ENQUEUE_RATE_WINDOW_SECS`].
pub const MCP_MAX_ENQUEUES_PER_WINDOW: usize = 30;

/// Reject MCP approve when fresh fee exceeds agent estimate by more than 10%.
pub const MCP_FEE_SPIKE_THRESHOLD_BPS: u64 = 1100;

/// Loopback port for MCP control plane (TUI listener).
pub const MCP_CONTROL_PORT: u16 = 8746;

/// Resolve the MCP control port (`VAUGHAN_MCP_PORT` override, else [`MCP_CONTROL_PORT`]).
///
/// Tests set `VAUGHAN_MCP_PORT` to an ephemeral port so they never collide with a
/// live Vaughan TUI on the default port.
pub fn mcp_control_port() -> u16 {
    std::env::var("VAUGHAN_MCP_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(MCP_CONTROL_PORT)
}

/// Type of proposed on-chain action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProposalType {
    NativeTransfer {
        to: Address,
        amount_wei: U256,
    },
    Erc20Transfer {
        token: Address,
        recipient: Address,
        amount: U256,
    },
    DexSwap {
        router: Address,
        path: Vec<Address>,
        amount_in: U256,
        min_amount_out: U256,
    },
    Batch7702 {
        target_count: usize,
        total_value: U256,
    },
    ContractCall {
        target: Address,
        function_name: Option<String>,
    },
    /// One step in a multi-step V3 LP deploy Brew (`propose_v3_lp_deploy`).
    LpDeployStep {
        job_id: String,
        step_label: String,
    },
    /// Fixed-supply ERC-20 deploy (testnet meme-coin launcher).
    TokenLaunch {
        name: String,
        symbol: String,
        supply_human: String,
    },
}

/// A structured transaction proposal from an external agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxProposal {
    pub proposal_id: String,
    pub proposal_type: ProposalType,
    pub to: Address,
    pub value_wei: U256,
    pub calldata: Bytes,
    pub gas_limit: u64,
    pub simulation_success: bool,
    pub estimated_fee_wei: Option<U256>,
    /// Human-readable agent text — **untrusted**; shown for context only.
    #[serde(alias = "llm_explanation")]
    pub explanation: String,
    /// Chain the proposal was built for; approve rejected on mismatch.
    #[serde(default)]
    pub chain_id: u64,
    /// Optional network id label (e.g. `pulsechain-testnet-v4`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<String>,
    /// Unix timestamp when the proposal was created.
    #[serde(default)]
    pub created_at_unix: u64,
}

impl TxProposal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        proposal_id: impl Into<String>,
        proposal_type: ProposalType,
        to: Address,
        value_wei: U256,
        calldata: Bytes,
        gas_limit: u64,
        simulation_success: bool,
        explanation: impl Into<String>,
    ) -> Self {
        Self {
            proposal_id: proposal_id.into(),
            proposal_type,
            to,
            value_wei,
            calldata,
            gas_limit,
            simulation_success,
            estimated_fee_wei: None,
            explanation: explanation.into(),
            chain_id: 0,
            network_id: None,
            created_at_unix: now_unix(),
        }
    }

    pub fn with_chain(mut self, chain_id: u64, network_id: Option<String>) -> Self {
        self.chain_id = chain_id;
        self.network_id = network_id;
        self
    }

    pub fn is_expired(&self) -> bool {
        now_unix().saturating_sub(self.created_at_unix) > PROPOSAL_TTL_SECS
    }
}

/// Lifecycle status of a queued proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProposalStatus {
    PendingUser,
    Approved { tx_hash: String },
    Rejected { reason: String },
    Expired,
}

/// On-disk envelope for a queued proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedProposal {
    pub proposal: TxProposal,
    pub source: String,
    pub hmac: String,
}

/// Machine-readable proposal/agent errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposalError {
    WalletLocked,
    NetworkMismatch { expected: u64, actual: u64 },
    ProposalExpired,
    SimulationReverted,
    UserRejected,
    MainnetBlocked,
    HmacInvalid,
    NotFound,
    QueueFull,
    InvalidProposalId,
    DuplicateProposalId,
    SessionRequired,
    RateLimited,
    FeeSpike,
    Io(String),
}

impl ProposalError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::WalletLocked => "wallet_locked",
            Self::NetworkMismatch { .. } => "network_mismatch",
            Self::ProposalExpired => "proposal_expired",
            Self::SimulationReverted => "simulation_reverted",
            Self::UserRejected => "user_rejected",
            Self::MainnetBlocked => "mainnet_blocked",
            Self::HmacInvalid => "hmac_invalid",
            Self::NotFound => "not_found",
            Self::QueueFull => "queue_full",
            Self::InvalidProposalId => "invalid_proposal_id",
            Self::DuplicateProposalId => "duplicate_proposal_id",
            Self::SessionRequired => "session_required",
            Self::RateLimited => "rate_limited",
            Self::FeeSpike => "fee_spike",
            Self::Io(_) => "io_error",
        }
    }
}

impl fmt::Display for ProposalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WalletLocked => write!(f, "wallet is locked"),
            Self::NetworkMismatch { expected, actual } => {
                write!(
                    f,
                    "network mismatch: proposal chain {expected}, active {actual}"
                )
            }
            Self::ProposalExpired => write!(f, "proposal expired"),
            Self::SimulationReverted => write!(f, "simulation reverted at approve time"),
            Self::UserRejected => write!(f, "user rejected proposal"),
            Self::MainnetBlocked => write!(f, "mainnet writes blocked for MCP"),
            Self::HmacInvalid => write!(f, "proposal integrity check failed"),
            Self::NotFound => write!(f, "proposal not found"),
            Self::QueueFull => write!(f, "too many pending proposals"),
            Self::InvalidProposalId => write!(f, "proposal_id must be 1-64 chars [a-zA-Z0-9_]"),
            Self::DuplicateProposalId => write!(f, "proposal_id already used"),
            Self::SessionRequired => write!(f, "MCP session token required to queue proposals"),
            Self::RateLimited => write!(f, "proposal enqueue rate limit exceeded"),
            Self::FeeSpike => write!(f, "network fee increased more than 10% since proposal"),
            Self::Io(msg) => write!(f, "{msg}"),
        }
    }
}

/// Validate agent `proposal_id` before using it as a filename component.
pub fn validate_proposal_id(id: &str) -> Result<(), ProposalError> {
    if id.is_empty() || id.len() > MAX_PROPOSAL_ID_LEN {
        return Err(ProposalError::InvalidProposalId);
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ProposalError::InvalidProposalId);
    }
    Ok(())
}

/// True when `fresh` exceeds `proposed` by more than 10% (agent estimate stale).
///
/// Fail-closed: a missing or zero proposed estimate counts as a spike, so a
/// hand-crafted proposal cannot skip the check by omitting `estimated_fee_wei`
/// (every built-in propose tool stamps it via `attach_estimated_fee`).
pub fn fee_spike_exceeds_threshold(proposed: Option<U256>, fresh: U256) -> bool {
    let Some(base) = proposed.filter(|v| !v.is_zero()) else {
        return true;
    };
    fresh.saturating_mul(U256::from(100u64))
        > base.saturating_mul(U256::from(MCP_FEE_SPIKE_THRESHOLD_BPS / 10))
}

impl std::error::Error for ProposalError {}

impl From<ProposalError> for WalletError {
    fn from(e: ProposalError) -> Self {
        Self::Other(format!("{}: {}", e.code(), e))
    }
}

/// Build an unsigned [`EvmTransaction`] from an approved proposal.
pub fn apply_proposal(
    wallet: &WalletState,
    proposal: &TxProposal,
) -> Result<EvmTransaction, WalletError> {
    let from = wallet.active_address()?.to_string();
    let net = wallet.networks().active();
    if proposal.chain_id != 0 && proposal.chain_id != net.chain_id {
        return Err(ProposalError::NetworkMismatch {
            expected: proposal.chain_id,
            actual: net.chain_id,
        }
        .into());
    }
    if proposal.is_expired() {
        return Err(ProposalError::ProposalExpired.into());
    }

    let svc = TransactionService::new();
    let data_hex = if proposal.calldata.is_empty() {
        String::new()
    } else {
        format!("0x{}", hex::encode(&proposal.calldata))
    };
    let to = format!("{:#x}", proposal.to);
    let value = proposal.value_wei.to_string();

    let tx = svc.build_contract_call(from, to, &data_hex, value, net.chain_id)?;
    let ChainTransaction::Evm(mut evm) = tx else {
        return Err(WalletError::InvalidTransaction(
            "expected EVM transaction".into(),
        ));
    };
    evm.gas_limit = Some(proposal.gas_limit);
    Ok(evm)
}

/// Apply agent-stamped total fee to an unsigned EVM tx before broadcast.
///
/// Avoids a fresh RPC `estimate_fee` on the UI thread when the proposal already
/// carries [`TxProposal::estimated_fee_wei`] (common for MCP / LP Brew steps).
pub fn apply_stamped_fee_to_evm(
    evm: &mut EvmTransaction,
    proposal: &TxProposal,
    priority_fee_wei: u64,
) {
    let Some(total) = proposal.estimated_fee_wei.filter(|w| !w.is_zero()) else {
        return;
    };
    let Some(gas) = evm.gas_limit.filter(|g| *g > 0) else {
        return;
    };
    let max = total / U256::from(gas);
    if max.is_zero() {
        return;
    }
    let tip = U256::from(priority_fee_wei).min(max);
    evm.max_fee_per_gas = Some(max.to_string());
    evm.max_priority_fee_per_gas = Some(tip.to_string());
}

/// File-backed proposal queue with HMAC integrity.
#[derive(Debug, Clone)]
pub struct ProposalQueue {
    root: PathBuf,
}

impl ProposalQueue {
    pub fn new(profile_dir: &Path) -> Self {
        Self {
            root: profile_dir.join("proposals"),
        }
    }

    fn pending_dir(&self) -> PathBuf {
        self.root.join("pending")
    }

    fn approved_dir(&self) -> PathBuf {
        self.root.join("approved")
    }

    fn rejected_dir(&self) -> PathBuf {
        self.root.join("rejected")
    }

    fn ensure_dirs(&self) -> Result<(), ProposalError> {
        for dir in [self.pending_dir(), self.approved_dir(), self.rejected_dir()] {
            fs::create_dir_all(&dir).map_err(|e| ProposalError::Io(e.to_string()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
            }
        }
        Ok(())
    }

    pub fn enqueue(
        &self,
        proposal: TxProposal,
        source: impl Into<String>,
        session_secret: &[u8],
    ) -> Result<QueuedProposal, ProposalError> {
        self.ensure_dirs()?;
        if session_secret.is_empty() {
            return Err(ProposalError::SessionRequired);
        }
        validate_proposal_id(&proposal.proposal_id)?;
        self.ensure_proposal_id_available(&proposal.proposal_id)?;
        let pending = self.list_pending()?;
        if pending.len() >= MAX_PENDING_PROPOSALS {
            return Err(ProposalError::QueueFull);
        }
        self.check_enqueue_rate()?;
        let source = source.into();
        let hmac = compute_proposal_hmac(session_secret, &source, &proposal)?;
        let queued = QueuedProposal {
            proposal,
            source,
            hmac,
        };
        let path = self
            .pending_dir()
            .join(format!("{}.json", queued.proposal.proposal_id));
        let json =
            serde_json::to_string_pretty(&queued).map_err(|e| ProposalError::Io(e.to_string()))?;
        write_atomic(&path, json.as_bytes())?;
        Ok(queued)
    }

    pub fn list_pending(&self) -> Result<Vec<QueuedProposal>, ProposalError> {
        self.ensure_dirs()?;
        let mut out = Vec::new();
        let dir = self.pending_dir();
        let entries = fs::read_dir(&dir).map_err(|e| ProposalError::Io(e.to_string()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // Oversized/planted files are skipped, not read (UI-thread DoS).
            let Ok(data) = read_capped(&path) else {
                continue;
            };
            if let Ok(queued) = serde_json::from_str::<QueuedProposal>(&data) {
                if !queued.proposal.is_expired() {
                    out.push(queued);
                }
            }
        }
        out.sort_by_key(|b| std::cmp::Reverse(b.proposal.created_at_unix));
        Ok(out)
    }

    pub fn get_pending(
        &self,
        proposal_id: &str,
        session_secret: &[u8],
    ) -> Result<QueuedProposal, ProposalError> {
        validate_proposal_id(proposal_id)?;
        let path = self.pending_dir().join(format!("{proposal_id}.json"));
        let data = read_capped(&path).map_err(|_| ProposalError::NotFound)?;
        let queued: QueuedProposal =
            serde_json::from_str(&data).map_err(|e| ProposalError::Io(e.to_string()))?;
        verify_proposal_hmac(session_secret, &queued)?;
        if queued.proposal.is_expired() {
            return Err(ProposalError::ProposalExpired);
        }
        Ok(queued)
    }

    pub fn mark_approved(
        &self,
        proposal_id: &str,
        tx_hash: &str,
        session_secret: &[u8],
    ) -> Result<(), ProposalError> {
        validate_proposal_id(proposal_id)?;
        self.ensure_dirs()?;
        let (proposal, source) = match self.get_pending(proposal_id, session_secret) {
            Ok(queued) => {
                let pending_path = self.pending_dir().join(format!("{proposal_id}.json"));
                let _ = fs::remove_file(&pending_path);
                (Some(queued.proposal), queued.source)
            }
            // Live IPC auto-exec never enqueued — still persist outcome for status polls.
            Err(_) => (None, "mcp".into()),
        };
        let record = serde_json::json!({
            "proposal_id": proposal_id,
            "proposal": proposal,
            "source": source,
            "status": { "status": "approved", "tx_hash": tx_hash },
        });
        let approved_path = self.approved_dir().join(format!("{proposal_id}.json"));
        let json =
            serde_json::to_string_pretty(&record).map_err(|e| ProposalError::Io(e.to_string()))?;
        write_atomic(&approved_path, json.as_bytes())?;
        append_history(&self.root, &record)?;
        Ok(())
    }

    pub fn mark_rejected(
        &self,
        proposal_id: &str,
        reason: &str,
        session_secret: &[u8],
    ) -> Result<(), ProposalError> {
        validate_proposal_id(proposal_id)?;
        let queued = self.get_pending(proposal_id, session_secret)?;
        let pending_path = self.pending_dir().join(format!("{proposal_id}.json"));
        let _ = fs::remove_file(&pending_path);
        let record = serde_json::json!({
            "proposal": queued.proposal,
            "source": queued.source,
            "status": { "status": "rejected", "reason": reason },
        });
        let rejected_path = self.rejected_dir().join(format!("{proposal_id}.json"));
        let json =
            serde_json::to_string_pretty(&record).map_err(|e| ProposalError::Io(e.to_string()))?;
        write_atomic(&rejected_path, json.as_bytes())?;
        append_history(&self.root, &record)?;
        Ok(())
    }

    /// Resolve lifecycle status: pending → approved/rejected files → expired/unknown.
    pub fn lookup_status(
        &self,
        proposal_id: &str,
        session_secret: &[u8],
    ) -> Result<ProposalStatus, ProposalError> {
        self.ensure_dirs()?;
        validate_proposal_id(proposal_id)?;
        match self.get_pending(proposal_id, session_secret) {
            Ok(queued) => {
                if queued.proposal.is_expired() {
                    return Ok(ProposalStatus::Expired);
                }
                return Ok(ProposalStatus::PendingUser);
            }
            Err(ProposalError::ProposalExpired) => return Ok(ProposalStatus::Expired),
            Err(ProposalError::NotFound) | Err(ProposalError::HmacInvalid) => {}
            Err(e) => return Err(e),
        }

        let approved_path = self.approved_dir().join(format!("{proposal_id}.json"));
        if approved_path.exists() {
            let data =
                fs::read_to_string(&approved_path).map_err(|e| ProposalError::Io(e.to_string()))?;
            let v: serde_json::Value =
                serde_json::from_str(&data).map_err(|e| ProposalError::Io(e.to_string()))?;
            let tx_hash = v
                .pointer("/status/tx_hash")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            return Ok(ProposalStatus::Approved { tx_hash });
        }

        let rejected_path = self.rejected_dir().join(format!("{proposal_id}.json"));
        if rejected_path.exists() {
            let data =
                fs::read_to_string(&rejected_path).map_err(|e| ProposalError::Io(e.to_string()))?;
            let v: serde_json::Value =
                serde_json::from_str(&data).map_err(|e| ProposalError::Io(e.to_string()))?;
            let reason = v
                .pointer("/status/reason")
                .and_then(|x| x.as_str())
                .unwrap_or("rejected")
                .to_string();
            return Ok(ProposalStatus::Rejected { reason });
        }

        Err(ProposalError::NotFound)
    }

    pub fn sweep_expired(&self) -> Result<usize, ProposalError> {
        self.ensure_dirs()?;
        let mut removed = 0usize;
        let dir = self.pending_dir();
        let entries = fs::read_dir(&dir).map_err(|e| ProposalError::Io(e.to_string()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(data) = read_capped(&path) {
                if let Ok(queued) = serde_json::from_str::<QueuedProposal>(&data) {
                    if queued.proposal.is_expired() {
                        let _ = fs::remove_file(&path);
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }

    /// Re-tag pending proposals after MCP session rotation (TUI unlock/restart).
    ///
    /// Agents enqueue with the session HMAC; a new loopback token invalidates the
    /// old tag without removing the proposal. Rebind lets the file queue surface
    /// again without asking the agent to re-propose.
    pub fn rebind_pending_session(&self, session_secret: &[u8]) -> Result<usize, ProposalError> {
        if session_secret.is_empty() {
            return Err(ProposalError::SessionRequired);
        }
        self.ensure_dirs()?;
        let pending = self.list_pending()?;
        let mut rebound = 0usize;
        for queued in pending {
            let mut updated = queued;
            updated.hmac =
                compute_proposal_hmac(session_secret, &updated.source, &updated.proposal)?;
            let path = self
                .pending_dir()
                .join(format!("{}.json", updated.proposal.proposal_id));
            let json = serde_json::to_string_pretty(&updated)
                .map_err(|e| ProposalError::Io(e.to_string()))?;
            write_atomic(&path, json.as_bytes())?;
            rebound += 1;
        }
        Ok(rebound)
    }

    fn ensure_proposal_id_available(&self, proposal_id: &str) -> Result<(), ProposalError> {
        let pending_path = self.pending_dir().join(format!("{proposal_id}.json"));
        if pending_path.is_file() {
            if let Ok(data) = fs::read_to_string(&pending_path) {
                if let Ok(queued) = serde_json::from_str::<QueuedProposal>(&data) {
                    if queued.proposal.is_expired() {
                        let _ = fs::remove_file(&pending_path);
                    } else {
                        return Err(ProposalError::DuplicateProposalId);
                    }
                } else {
                    return Err(ProposalError::DuplicateProposalId);
                }
            }
        }
        for dir in [self.approved_dir(), self.rejected_dir()] {
            if dir.join(format!("{proposal_id}.json")).exists() {
                return Err(ProposalError::DuplicateProposalId);
            }
        }
        Ok(())
    }

    fn check_enqueue_rate(&self) -> Result<(), ProposalError> {
        let path = self.root.join(".enqueue_rate.json");
        let now = now_unix();
        let mut stamps: Vec<u64> = if path.is_file() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        stamps.retain(|t| now.saturating_sub(*t) < MCP_ENQUEUE_RATE_WINDOW_SECS);
        if stamps.len() >= MCP_MAX_ENQUEUES_PER_WINDOW {
            return Err(ProposalError::RateLimited);
        }
        stamps.push(now);
        let json = serde_json::to_string(&stamps).map_err(|e| ProposalError::Io(e.to_string()))?;
        write_atomic(&path, json.as_bytes())?;
        Ok(())
    }
}

/// MCP session token persisted beside the profile vault.
#[derive(Clone)]
pub struct McpSessionToken {
    token: String,
}

impl std::fmt::Debug for McpSessionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpSessionToken(REDACTED)")
    }
}

impl McpSessionToken {
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        Self {
            token: hex::encode(bytes),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.token
    }

    pub fn write(&self, profile_dir: &Path) -> Result<(), WalletError> {
        fs::create_dir_all(profile_dir).map_err(|e| WalletError::Io(e.to_string()))?;
        let path = profile_dir.join("mcp.session");
        write_atomic(&path, self.token.as_bytes()).map_err(|e| WalletError::Io(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn read(profile_dir: &Path) -> Result<Option<String>, WalletError> {
        let path = profile_dir.join("mcp.session");
        match fs::read_to_string(&path) {
            Ok(s) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
            Ok(_) => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(WalletError::Io(e.to_string())),
        }
    }

    pub fn invalidate(profile_dir: &Path) -> Result<(), WalletError> {
        let path = profile_dir.join("mcp.session");
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(WalletError::Io(e.to_string())),
        }
    }
}

/// Returns true when mainnet MCP writes are allowed.
pub fn mcp_mainnet_writes_allowed() -> bool {
    std::env::var("VAUGHAN_MCP_ALLOW_MAINNET")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Reject mainnet write proposals unless explicitly allowed.
///
/// Gates on the **active** network's `is_testnet` flag, not the proposal's
/// `chain_id`: execution always happens on the active chain (a `chain_id` of
/// `0` means "active chain"), so a denylist of known mainnet ids would be
/// bypassable by omitting the field.
pub fn guard_mainnet_write(is_testnet: bool) -> Result<(), ProposalError> {
    if is_testnet || mcp_mainnet_writes_allowed() {
        Ok(())
    } else {
        Err(ProposalError::MainnetBlocked)
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), ProposalError> {
    // Unique tmp name: concurrent writers (e.g. the enqueue rate limiter)
    // must not share a scratch file.
    static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = format!(
        "tmp.{:x}.{:x}",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let tmp = path.with_extension(unique);
    {
        let mut file = fs::File::create(&tmp).map_err(|e| ProposalError::Io(e.to_string()))?;
        // Restrict before writing so the file is never readable between
        // create and rename (session tokens ride this path).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
        }
        file.write_all(data)
            .map_err(|e| ProposalError::Io(e.to_string()))?;
        file.sync_all()
            .map_err(|e| ProposalError::Io(e.to_string()))?;
    }
    fs::rename(&tmp, path).map_err(|e| ProposalError::Io(e.to_string()))?;
    Ok(())
}

/// Largest proposal/queue file we will read — anything bigger is skipped, so
/// a planted multi-GB file cannot freeze the caller.
const MAX_QUEUE_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Read a queue file, refusing oversized ones (planted-file DoS guard).
fn read_capped(path: &Path) -> Result<String, ProposalError> {
    let meta = fs::metadata(path).map_err(|e| ProposalError::Io(e.to_string()))?;
    if meta.len() > MAX_QUEUE_FILE_BYTES {
        return Err(ProposalError::Io(format!(
            "queue file too large ({} bytes)",
            meta.len()
        )));
    }
    fs::read_to_string(path).map_err(|e| ProposalError::Io(e.to_string()))
}

fn append_history(root: &Path, record: &serde_json::Value) -> Result<(), ProposalError> {
    fs::create_dir_all(root).map_err(|e| ProposalError::Io(e.to_string()))?;
    let path = root.join("history.jsonl");
    let mut opts = fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(&path)
        .map_err(|e| ProposalError::Io(e.to_string()))?;
    let line = serde_json::to_string(record).map_err(|e| ProposalError::Io(e.to_string()))?;
    writeln!(file, "{line}").map_err(|e| ProposalError::Io(e.to_string()))?;
    Ok(())
}

/// HMAC covers the `source` label as well as the proposal body, so a queued
/// file cannot be relabeled (e.g. `unknown-script` → `cursor`) without
/// invalidating the tag.
fn compute_proposal_hmac(
    secret: &[u8],
    source: &str,
    proposal: &TxProposal,
) -> Result<String, ProposalError> {
    let bytes =
        serde_json::to_vec(&(source, proposal)).map_err(|e| ProposalError::Io(e.to_string()))?;
    Ok(hex::encode(hmac_sha256(secret, &bytes)))
}

fn verify_proposal_hmac(secret: &[u8], queued: &QueuedProposal) -> Result<(), ProposalError> {
    let expected = compute_proposal_hmac(secret, &queued.source, &queued.proposal)?;
    if constant_time_eq(expected.as_bytes(), queued.hmac.as_bytes()) {
        Ok(())
    } else {
        Err(ProposalError::HmacInvalid)
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// JSON shape for MCP / IPC proposal status polls.
pub fn proposal_status_json(proposal_id: &str, status: &ProposalStatus) -> serde_json::Value {
    match status {
        ProposalStatus::PendingUser => serde_json::json!({
            "proposal_id": proposal_id,
            "status": "pending_user",
        }),
        ProposalStatus::Approved { tx_hash } => serde_json::json!({
            "proposal_id": proposal_id,
            "status": "approved",
            "tx_hash": tx_hash,
        }),
        ProposalStatus::Rejected { reason } => serde_json::json!({
            "proposal_id": proposal_id,
            "status": "rejected",
            "reason": reason,
        }),
        ProposalStatus::Expired => serde_json::json!({
            "proposal_id": proposal_id,
            "status": "expired",
        }),
    }
}

/// HMAC-SHA256 using only the `sha2` crate (allowlist-compliant).
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let hash = Sha256::digest(key);
        key_block[..32].copy_from_slice(&hash);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }
    let inner = Sha256::digest([ipad.as_slice(), message].concat());
    let outer = Sha256::digest([opad.as_slice(), &inner[..]].concat());
    outer.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::hd_wallet::validate_mnemonic;
    use alloy::primitives::address;
    use secrecy::SecretString;
    use tempfile::TempDir;

    #[test]
    fn apply_proposal_native_transfer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("wallet.json");
        let mut wallet = WalletState::load(path).unwrap();
        wallet
            .create(
                &SecretString::from("TestPassword1!".to_string()),
                validate_mnemonic(
                    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
                )
                .unwrap(),
            )
            .unwrap();
        wallet.set_active_network("pulsechain-testnet-v4").unwrap();

        let recipient = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
        let proposal = TxProposal::new(
            "prop_test",
            ProposalType::NativeTransfer {
                to: recipient,
                amount_wei: U256::from(1_000_000_000_000_000_000u64),
            },
            recipient,
            U256::from(1_000_000_000_000_000_000u64),
            Bytes::new(),
            21_000,
            true,
            "test transfer",
        )
        .with_chain(943, Some("pulsechain-testnet-v4".into()));

        let evm = apply_proposal(&wallet, &proposal).unwrap();
        assert_eq!(evm.to, format!("{recipient:#x}"));
        assert_eq!(evm.value, "1000000000000000000");
        assert_eq!(evm.chain_id, 943);
        assert_eq!(evm.gas_limit, Some(21_000));
    }

    #[test]
    fn proposal_queue_roundtrip_and_hmac() {
        let dir = TempDir::new().unwrap();
        let queue = ProposalQueue::new(dir.path());
        let secret = b"test-session-secret";
        let proposal = TxProposal::new(
            "prop_abc",
            ProposalType::ContractCall {
                target: address!("1111111111111111111111111111111111111111"),
                function_name: Some("transfer".into()),
            },
            address!("1111111111111111111111111111111111111111"),
            U256::ZERO,
            Bytes::from_static(&[0x12, 0x34]),
            65_000,
            true,
            "call test",
        );

        let queued = queue.enqueue(proposal.clone(), "cursor", secret).unwrap();
        assert_eq!(queued.proposal.proposal_id, "prop_abc");

        let loaded = queue.get_pending("prop_abc", secret).unwrap();
        assert_eq!(loaded.proposal, proposal);

        let pending = queue.list_pending().unwrap();
        assert_eq!(pending.len(), 1);

        queue.mark_approved("prop_abc", "0xdead", secret).unwrap();
        assert!(queue.get_pending("prop_abc", secret).is_err());
        match queue.lookup_status("prop_abc", secret).unwrap() {
            ProposalStatus::Approved { tx_hash } => assert_eq!(tx_hash, "0xdead"),
            other => panic!("expected approved, got {other:?}"),
        }
    }

    #[test]
    fn mark_approved_without_pending_still_records_status() {
        let dir = TempDir::new().unwrap();
        let queue = ProposalQueue::new(dir.path());
        let secret = b"live-ipc-secret";
        queue.mark_approved("prop_live", "0xcafe", secret).unwrap();
        match queue.lookup_status("prop_live", secret).unwrap() {
            ProposalStatus::Approved { tx_hash } => assert_eq!(tx_hash, "0xcafe"),
            other => panic!("expected approved, got {other:?}"),
        }
    }

    #[test]
    fn apply_proposal_rejects_network_mismatch() {
        let dir = TempDir::new().unwrap();
        let mut wallet = WalletState::load(dir.path().join("wallet.json")).unwrap();
        let mnemonic =
            validate_mnemonic("test test test test test test test test test test test junk")
                .unwrap();
        wallet
            .create(&SecretString::from("BombProof123!".to_string()), mnemonic)
            .unwrap();
        wallet.set_active_network("pulsechain-testnet-v4").unwrap();

        let recipient = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
        let proposal = TxProposal::new(
            "prop_mismatch",
            ProposalType::NativeTransfer {
                to: recipient,
                amount_wei: U256::from(1u64),
            },
            recipient,
            U256::from(1u64),
            Bytes::new(),
            21_000,
            true,
            "wrong chain",
        )
        .with_chain(369, Some("pulsechain".into()));

        let err = apply_proposal(&wallet, &proposal).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("network mismatch") || msg.contains("369"),
            "unexpected: {msg}"
        );
    }

    #[test]
    fn guard_mainnet_write_gates() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("VAUGHAN_MCP_ALLOW_MAINNET").ok();

        std::env::remove_var("VAUGHAN_MCP_ALLOW_MAINNET");
        // Any non-testnet active network blocks writes — a proposal that omits
        // chain_id (0 = active chain) cannot bypass the guard.
        assert!(matches!(
            guard_mainnet_write(false),
            Err(ProposalError::MainnetBlocked)
        ));
        assert!(guard_mainnet_write(true).is_ok());

        std::env::set_var("VAUGHAN_MCP_ALLOW_MAINNET", "1");
        assert!(guard_mainnet_write(false).is_ok());

        match prev {
            Some(v) => std::env::set_var("VAUGHAN_MCP_ALLOW_MAINNET", v),
            None => std::env::remove_var("VAUGHAN_MCP_ALLOW_MAINNET"),
        }
    }

    #[test]
    fn proposal_queue_rejects_when_full() {
        let dir = TempDir::new().unwrap();
        let queue = ProposalQueue::new(dir.path());
        let secret = b"queue-full-secret-test!!!!!!";
        for i in 0..MAX_PENDING_PROPOSALS {
            let proposal = TxProposal::new(
                format!("prop_full_{i}"),
                ProposalType::NativeTransfer {
                    to: address!("1111111111111111111111111111111111111111"),
                    amount_wei: U256::from(1u64),
                },
                address!("1111111111111111111111111111111111111111"),
                U256::from(1u64),
                Bytes::new(),
                21_000,
                true,
                "fill queue",
            );
            queue.enqueue(proposal, "test", secret).unwrap();
        }
        let overflow = TxProposal::new(
            "prop_overflow",
            ProposalType::NativeTransfer {
                to: address!("2222222222222222222222222222222222222222"),
                amount_wei: U256::from(1u64),
            },
            address!("2222222222222222222222222222222222222222"),
            U256::from(1u64),
            Bytes::new(),
            21_000,
            true,
            "one too many",
        );
        assert!(matches!(
            queue.enqueue(overflow, "test", secret),
            Err(ProposalError::QueueFull)
        ));
    }

    #[test]
    fn validate_proposal_id_rejects_path_traversal() {
        assert!(matches!(
            validate_proposal_id("../evil"),
            Err(ProposalError::InvalidProposalId)
        ));
        assert!(validate_proposal_id("prop_ok_123").is_ok());
    }

    #[test]
    fn enqueue_rejects_empty_session_secret() {
        let dir = TempDir::new().unwrap();
        let queue = ProposalQueue::new(dir.path());
        let proposal = TxProposal::new(
            "prop_nosecret",
            ProposalType::NativeTransfer {
                to: address!("1111111111111111111111111111111111111111"),
                amount_wei: U256::from(1u64),
            },
            address!("1111111111111111111111111111111111111111"),
            U256::from(1u64),
            Bytes::new(),
            21_000,
            true,
            "test",
        );
        assert!(matches!(
            queue.enqueue(proposal, "test", b""),
            Err(ProposalError::SessionRequired)
        ));
    }

    #[test]
    fn enqueue_rejects_duplicate_proposal_id() {
        let dir = TempDir::new().unwrap();
        let queue = ProposalQueue::new(dir.path());
        let secret = b"dup-test-secret!!!!!!!!!!!!!!";
        let proposal = TxProposal::new(
            "prop_dup",
            ProposalType::NativeTransfer {
                to: address!("1111111111111111111111111111111111111111"),
                amount_wei: U256::from(1u64),
            },
            address!("1111111111111111111111111111111111111111"),
            U256::from(1u64),
            Bytes::new(),
            21_000,
            true,
            "dup",
        );
        queue.enqueue(proposal.clone(), "a", secret).unwrap();
        assert!(matches!(
            queue.enqueue(proposal, "b", secret),
            Err(ProposalError::DuplicateProposalId)
        ));
    }

    #[test]
    fn fee_spike_threshold() {
        let base = U256::from(1000u64);
        assert!(!fee_spike_exceeds_threshold(
            Some(base),
            U256::from(1100u64)
        ));
        assert!(fee_spike_exceeds_threshold(Some(base), U256::from(1101u64)));
        // Fail-closed: no agent estimate counts as a spike.
        assert!(fee_spike_exceeds_threshold(None, U256::from(9999u64)));
        assert!(fee_spike_exceeds_threshold(
            Some(U256::ZERO),
            U256::from(9999u64)
        ));
    }

    #[test]
    fn proposal_queue_rejects_tampered_hmac() {
        let dir = TempDir::new().unwrap();
        let queue = ProposalQueue::new(dir.path());
        let secret = b"test-session-secret";
        let proposal = TxProposal::new(
            "prop_tamper",
            ProposalType::NativeTransfer {
                to: address!("2222222222222222222222222222222222222222"),
                amount_wei: U256::from(1u64),
            },
            address!("2222222222222222222222222222222222222222"),
            U256::from(1u64),
            Bytes::new(),
            21_000,
            true,
            "tamper test",
        );
        queue.enqueue(proposal, "cli", secret).unwrap();

        let path = queue.pending_dir().join("prop_tamper.json");
        let mut doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        doc["hmac"] = serde_json::json!("00");
        fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

        assert!(matches!(
            queue.get_pending("prop_tamper", secret),
            Err(ProposalError::HmacInvalid)
        ));
    }
}
