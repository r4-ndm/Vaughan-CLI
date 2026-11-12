//! Sentient session policy — user-owned circuit-breaker dials.
//!
//! Persisted as `sentient-policy.toml` beside the profile wallet. Agents may
//! **propose** changes; only the human (via `/policy` or a future approval
//! card) writes the file. Safe defaults match [`CircuitBreakerConfig::default`].

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};

use super::circuit_breaker::CircuitBreakerConfig;
use crate::error::AgentError;

/// Filename next to `wallet.json` / `agent.toml`.
pub const SENTIENT_POLICY_TOML: &str = "sentient-policy.toml";

/// Legacy filename (pre-rename); still loaded when the new file is absent.
pub const LEGACY_DEGEN_POLICY_TOML: &str = "degen-policy.toml";

fn policy_file_path(dir: &Path) -> Option<PathBuf> {
    let sentient = dir.join(SENTIENT_POLICY_TOML);
    if sentient.is_file() {
        return Some(sentient);
    }
    let legacy = dir.join(LEGACY_DEGEN_POLICY_TOML);
    if legacy.is_file() {
        return Some(legacy);
    }
    None
}

/// How hard the Rust breakers enforce limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EnforcementMode {
    /// Reject oversize / overslippage; trip on gas / consecutive errors (default).
    #[default]
    Enforced,
    /// Allow trades that would fail under Enforced; log a warning (lab / tuning).
    WarnOnly,
    /// Skip position / slippage / gas / error tripwires. The Ctrl+K kill
    /// switch still trips the session breaker.
    /// Requires [`AgentSessionPolicy::acknowledge_unsafe`].
    Disabled,
}

impl EnforcementMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enforced => "enforced",
            Self::WarnOnly => "warn-only",
            Self::Disabled => "disabled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "enforced" | "on" | "strict" => Some(Self::Enforced),
            "warn-only" | "warn" | "warning" => Some(Self::WarnOnly),
            "disabled" | "off" | "unsafe" => Some(Self::Disabled),
            _ => None,
        }
    }
}

/// User-editable Sentient guardrails (burner profile only).
///
/// `deny_unknown_fields`: a typo'd key (or a stale legacy file) must fail
/// closed at load time, never silently fall back to weaker defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentSessionPolicy {
    /// Enforcement posture.
    #[serde(default)]
    pub enforcement: EnforcementMode,
    /// Must be true to persist or apply [`EnforcementMode::Disabled`].
    #[serde(default)]
    pub acknowledge_unsafe: bool,
    /// Max % of native balance per trade (1–100).
    #[serde(default = "default_max_position_pct")]
    pub max_position_pct: u8,
    /// Max % of an asset balance per approval (1–100). Defaults to max_position_pct if not set.
    #[serde(default)]
    pub max_approve_pct: Option<u8>,
    /// Max % of an asset's window-start balance that may leave the account per 24h window.
    #[serde(default = "default_daily_cap_pct")]
    pub daily_cap_pct: u8,
    /// Addresses the sentient agent may transfer native/tokens TO. Empty = all transfers refused.
    #[serde(default)]
    pub allow_transfer_to: Vec<String>,
    /// Max slippage in basis points.
    #[serde(default = "default_max_slippage_bps")]
    pub max_slippage_bps: u32,
    /// Session gas ceiling in wei (decimal string in TOML via serde U256? — store as string).
    #[serde(default = "default_max_session_gas_wei_str")]
    pub max_session_gas_wei: String,
    /// Consecutive failures before trip (ignored when Disabled).
    #[serde(default = "default_max_consecutive_errors")]
    pub max_consecutive_errors: u32,
    /// Min agreeing RPCs for reserves quorum (1 = single-RPC lab).
    #[serde(default = "default_required_rpc_quorum")]
    pub required_rpc_quorum: usize,
}

fn default_max_position_pct() -> u8 {
    10
}
fn default_daily_cap_pct() -> u8 {
    20
}
fn default_max_slippage_bps() -> u32 {
    100
}
fn default_max_session_gas_wei_str() -> String {
    "50000000000000000".into() // 0.05 native
}
fn default_max_consecutive_errors() -> u32 {
    3
}
fn default_required_rpc_quorum() -> usize {
    2
}

impl Default for AgentSessionPolicy {
    fn default() -> Self {
        Self {
            enforcement: EnforcementMode::Enforced,
            acknowledge_unsafe: false,
            max_position_pct: default_max_position_pct(),
            max_approve_pct: None,
            daily_cap_pct: default_daily_cap_pct(),
            allow_transfer_to: Vec::new(),
            max_slippage_bps: default_max_slippage_bps(),
            max_session_gas_wei: default_max_session_gas_wei_str(),
            max_consecutive_errors: default_max_consecutive_errors(),
            required_rpc_quorum: default_required_rpc_quorum(),
        }
    }
}

impl AgentSessionPolicy {
    /// Effective maximum percentage of asset balance for approvals (default: max_position_pct).
    pub fn max_approve_pct(&self) -> u8 {
        self.max_approve_pct.unwrap_or(self.max_position_pct)
    }

    /// Compute a deterministic SHA-256 hex digest of this policy.
    pub fn digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        hex::encode(Sha256::digest(&bytes))
    }

    /// Validate ranges and unsafe acknowledgement.
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.max_position_pct == 0 || self.max_position_pct > 100 {
            return Err(AgentError::InvalidToolCall(
                "max_position_pct must be 1..=100".into(),
            ));
        }
        if let Some(pct) = self.max_approve_pct {
            if pct == 0 || pct > 100 {
                return Err(AgentError::InvalidToolCall(
                    "max_approve_pct must be 1..=100".into(),
                ));
            }
        }
        if self.daily_cap_pct == 0 || self.daily_cap_pct > 100 {
            return Err(AgentError::InvalidToolCall(
                "daily_cap_pct must be 1..=100".into(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for entry in &self.allow_transfer_to {
            if entry.trim().parse::<Address>().is_err() {
                return Err(AgentError::InvalidToolCall(format!(
                    "invalid address in allow_transfer_to: `{entry}`"
                )));
            }
            let lower = entry.trim().to_ascii_lowercase();
            if !seen.insert(lower) {
                return Err(AgentError::InvalidToolCall(format!(
                    "duplicate address in allow_transfer_to: `{entry}`"
                )));
            }
        }
        if self.max_slippage_bps > 10_000 {
            return Err(AgentError::InvalidToolCall(
                "max_slippage_bps must be ≤ 10000 (100%)".into(),
            ));
        }
        if self.max_consecutive_errors == 0 {
            return Err(AgentError::InvalidToolCall(
                "max_consecutive_errors must be ≥ 1".into(),
            ));
        }
        if self.required_rpc_quorum == 0 {
            return Err(AgentError::InvalidToolCall(
                "required_rpc_quorum must be ≥ 1".into(),
            ));
        }
        let _gas = parse_wei(&self.max_session_gas_wei)?;
        if self.enforcement == EnforcementMode::Disabled && !self.acknowledge_unsafe {
            return Err(AgentError::InvalidToolCall(
                "enforcement=disabled requires acknowledge_unsafe = true \
                 (type `/policy confirm-unsafe` then `/policy set enforcement disabled`)"
                    .into(),
            ));
        }
        Ok(())
    }

    /// Apply a single key/value (same keys as `/policy set` and CLI).
    pub fn set_field(&mut self, key: &str, value: &str) -> Result<(), AgentError> {
        match key.trim().to_ascii_lowercase().as_str() {
            "enforcement" => {
                let mode = EnforcementMode::parse(value).ok_or_else(|| {
                    AgentError::InvalidToolCall(
                        "enforcement: use enforced | warn-only | disabled".into(),
                    )
                })?;
                self.enforcement = mode;
            }
            "max_position_pct" => {
                self.max_position_pct = value.parse().map_err(|_| {
                    AgentError::InvalidToolCall("max_position_pct: need integer 1..=100".into())
                })?;
            }
            "daily_cap_pct" => {
                self.daily_cap_pct = value.parse().map_err(|_| {
                    AgentError::InvalidToolCall("daily_cap_pct: need integer 1..=100".into())
                })?;
            }
            "allow_transfer_to" => {
                self.allow_transfer_to = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "max_slippage_bps" => {
                self.max_slippage_bps = value.parse().map_err(|_| {
                    AgentError::InvalidToolCall("max_slippage_bps: need integer".into())
                })?;
            }
            "max_session_gas_wei" => {
                self.max_session_gas_wei = value.trim().to_string();
            }
            "max_consecutive_errors" => {
                self.max_consecutive_errors = value.parse().map_err(|_| {
                    AgentError::InvalidToolCall("max_consecutive_errors: need integer ≥ 1".into())
                })?;
            }
            "required_rpc_quorum" => {
                self.required_rpc_quorum = value.parse().map_err(|_| {
                    AgentError::InvalidToolCall("required_rpc_quorum: need integer ≥ 1".into())
                })?;
            }
            "acknowledge_unsafe" => {
                self.acknowledge_unsafe = matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                );
            }
            _ => {
                return Err(AgentError::InvalidToolCall(format!(
                    "unknown policy key `{key}`"
                )));
            }
        }
        Ok(())
    }

    /// Map into the runtime breaker config (quorum may still be clamped by available RPCs).
    pub fn to_breaker_config(&self) -> Result<CircuitBreakerConfig, AgentError> {
        self.validate()?;
        Ok(CircuitBreakerConfig {
            max_position_pct: self.max_position_pct,
            max_slippage_bps: self.max_slippage_bps,
            max_session_gas_wei: parse_wei(&self.max_session_gas_wei)?,
            max_consecutive_errors: self.max_consecutive_errors,
            required_rpc_quorum: self.required_rpc_quorum,
            enforcement: self.enforcement,
        })
    }
}

/// AI-proposed policy change awaiting human `[a]` / `[d]` (does not sign txs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyProposal {
    pub proposal_id: String,
    pub llm_explanation: String,
    /// Policy before the change.
    pub before: AgentSessionPolicy,
    /// Policy after applying the proposed patch.
    pub after: AgentSessionPolicy,
    /// Human-readable diff lines (`max_slippage_bps: 100 → 500`).
    pub changes: Vec<String>,
}

/// Build a [`PolicyProposal`] from a list of key/value patches on `before`.
pub fn build_policy_proposal(
    before: AgentSessionPolicy,
    patches: &[(String, String)],
    explanation: impl Into<String>,
) -> Result<PolicyProposal, AgentError> {
    if patches.is_empty() {
        return Err(AgentError::InvalidToolCall(
            "policy proposal needs at least one change".into(),
        ));
    }
    let explanation = explanation.into();
    let mut after = before.clone();
    let mut changes = Vec::new();
    for (key, value) in patches {
        let old = match key.as_str() {
            "enforcement" => before.enforcement.as_str().to_string(),
            "max_position_pct" => before.max_position_pct.to_string(),
            "daily_cap_pct" => before.daily_cap_pct.to_string(),
            "allow_transfer_to" => before.allow_transfer_to.join(", "),
            "max_slippage_bps" => before.max_slippage_bps.to_string(),
            "max_session_gas_wei" => before.max_session_gas_wei.clone(),
            "max_consecutive_errors" => before.max_consecutive_errors.to_string(),
            "required_rpc_quorum" => before.required_rpc_quorum.to_string(),
            "acknowledge_unsafe" => before.acknowledge_unsafe.to_string(),
            _ => "?".into(),
        };
        after.set_field(key, value)?;
        changes.push(format!("{key}: {old} → {value}"));
    }
    // Two-step guard: enforcement=disabled only if ack was already true before this card.
    if after.enforcement == EnforcementMode::Disabled && !before.acknowledge_unsafe {
        return Err(AgentError::InvalidToolCall(
            "cannot disable breakers in one card: human must run /policy confirm-unsafe \
             (or `vaughan policy confirm-unsafe`) first, then propose enforcement=disabled"
                .into(),
        ));
    }
    after.validate()?;
    // Stable-enough id for the card (not a secret).
    let digest = {
        let mut n: u32 = explanation.len() as u32;
        for c in &changes {
            for b in c.bytes() {
                n = n.wrapping_mul(31).wrapping_add(u32::from(b));
            }
        }
        n
    };
    Ok(PolicyProposal {
        proposal_id: format!("pol-{digest:08x}"),
        llm_explanation: explanation,
        before,
        after,
        changes,
    })
}

fn parse_wei(s: &str) -> Result<U256, AgentError> {
    let t = s.trim();
    U256::from_str(t)
        .or_else(|_| U256::from_str_radix(t.trim_start_matches("0x"), 16))
        .map_err(|_| AgentError::InvalidToolCall(format!("invalid max_session_gas_wei: {t}")))
}

/// Load policy from `dir/sentient-policy.toml` (or legacy `degen-policy.toml`), or defaults if missing.
///
/// Non-default enforcement is logged loudly: the file is the only guardrail
/// between an agent and the vault, so a downgrade must never be silent.
pub fn load_policy(dir: &Path) -> Result<AgentSessionPolicy, AgentError> {
    let Some(path) = policy_file_path(dir) else {
        return Ok(AgentSessionPolicy::default());
    };
    crate::sentient::file_guard::assert_owner_only_file(&path)?;
    let raw = fs::read_to_string(&path).map_err(|e| {
        AgentError::ProviderError(format!("failed to read {}: {e}", path.display()))
    })?;
    let policy: AgentSessionPolicy = toml::from_str(&raw).map_err(|e| {
        AgentError::ProviderError(format!(
            "invalid {}: {e}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("policy")
        ))
    })?;
    policy.validate()?;
    if policy.max_position_pct == 100 {
        tracing::warn!(
            target: "vaughan_agent::sentient",
            "per-trade cap 100% — the 24h cap is the only bound"
        );
    }
    match policy.enforcement {
        EnforcementMode::Enforced => {}
        EnforcementMode::WarnOnly => tracing::warn!(
            target: "vaughan_agent::sentient",
            "sentient policy enforcement=warn-only: breaker breaches are allowed, only logged"
        ),
        EnforcementMode::Disabled => tracing::warn!(
            target: "vaughan_agent::sentient",
            "sentient policy enforcement=DISABLED: agent trades unbounded (acknowledge_unsafe set)"
        ),
    }
    Ok(policy)
}

/// Atomically write `sentient-policy.toml` after validation (0600 — the file
/// is a fund-safety control, not casual config).
pub fn save_policy(dir: &Path, policy: &AgentSessionPolicy) -> Result<(), AgentError> {
    crate::paths::assert_signer_store_writable(dir)?;
    policy.validate()?;
    fs::create_dir_all(dir).map_err(|e| {
        AgentError::ProviderError(format!("failed to create {}: {e}", dir.display()))
    })?;
    let path = dir.join(SENTIENT_POLICY_TOML);
    let tmp = dir.join(format!(".{SENTIENT_POLICY_TOML}.tmp"));
    let body = toml::to_string_pretty(policy)
        .map_err(|e| AgentError::ProviderError(format!("serialize policy: {e}")))?;
    {
        use std::io::Write;
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(|e| {
            AgentError::ProviderError(format!("failed to write {}: {e}", tmp.display()))
        })?;
        f.write_all(body.as_bytes()).map_err(|e| {
            AgentError::ProviderError(format!("failed to write {}: {e}", tmp.display()))
        })?;
    }
    fs::rename(&tmp, &path).map_err(|e| {
        AgentError::ProviderError(format!("failed to replace {}: {e}", path.display()))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Resolve breaker config for a session: file policy + available RPC count.
pub fn breaker_config_for_session(
    dir: Option<&Path>,
    rpc_count: usize,
) -> Result<CircuitBreakerConfig, AgentError> {
    let mut policy = match dir {
        Some(d) => load_policy(d)?,
        None => AgentSessionPolicy::default(),
    };
    // Cannot require more RPCs than configured endpoints. Clamping is loud:
    // quorum validation only runs in the (dormant) SentientTrader path, so a
    // silently-clamped dial would over-promise multi-RPC verification.
    let available = rpc_count.max(1);
    if policy.required_rpc_quorum > available {
        tracing::warn!(
            target: "vaughan_agent::sentient",
            required = policy.required_rpc_quorum,
            available,
            "required_rpc_quorum clamped to available RPC endpoints"
        );
        policy.required_rpc_quorum = available;
    }
    policy.to_breaker_config()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_roundtrip_toml() {
        let dir = tempdir().unwrap();
        let p = AgentSessionPolicy::default();
        save_policy(dir.path(), &p).unwrap();
        let loaded = load_policy(dir.path()).unwrap();
        assert_eq!(loaded.enforcement, EnforcementMode::Enforced);
        assert_eq!(loaded.max_slippage_bps, 100);
        assert!(!loaded.acknowledge_unsafe);
        assert_eq!(loaded.max_approve_pct, None);
        assert_eq!(loaded.max_approve_pct(), loaded.max_position_pct);
    }

    #[test]
    fn max_approve_pct_override_and_validation() {
        let mut p = AgentSessionPolicy {
            max_position_pct: 15,
            max_approve_pct: Some(25),
            ..Default::default()
        };
        assert_eq!(p.max_approve_pct(), 25);
        assert!(p.validate().is_ok());

        p.max_approve_pct = Some(0);
        assert!(p.validate().is_err());

        p.max_approve_pct = Some(101);
        assert!(p.validate().is_err());

        p.max_approve_pct = None;
        assert_eq!(p.max_approve_pct(), 15);
        assert!(p.validate().is_ok());
    }

    #[test]
    fn policy_digest_deterministic() {
        let p1 = AgentSessionPolicy::default();
        let p2 = AgentSessionPolicy::default();
        assert_eq!(p1.digest(), p2.digest());
        assert_eq!(p1.digest().len(), 64);

        let p3 = AgentSessionPolicy {
            max_slippage_bps: 200,
            ..Default::default()
        };
        assert_ne!(p1.digest(), p3.digest());
    }

    #[test]
    fn disabled_requires_ack() {
        let mut p = AgentSessionPolicy {
            enforcement: EnforcementMode::Disabled,
            ..Default::default()
        };
        assert!(p.validate().is_err());
        p.acknowledge_unsafe = true;
        assert!(p.validate().is_ok());
    }

    #[test]
    fn set_slippage_persists() {
        let dir = tempdir().unwrap();
        let p = AgentSessionPolicy {
            max_slippage_bps: 500,
            ..Default::default()
        };
        save_policy(dir.path(), &p).unwrap();
        assert_eq!(load_policy(dir.path()).unwrap().max_slippage_bps, 500);
    }

    #[test]
    fn policy_proposal_diff() {
        let before = AgentSessionPolicy::default();
        let prop = build_policy_proposal(
            before,
            &[("max_slippage_bps".into(), "500".into())],
            "test wider slippage",
        )
        .unwrap();
        assert_eq!(prop.after.max_slippage_bps, 500);
        assert!(prop.changes[0].contains("100 → 500"));
    }

    #[test]
    fn reject_bundled_ack_and_disable() {
        let before = AgentSessionPolicy::default();
        let err = build_policy_proposal(
            before,
            &[
                ("acknowledge_unsafe".into(), "true".into()),
                ("enforcement".into(), "disabled".into()),
            ],
            "one-click disable",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("confirm-unsafe") || err.to_string().contains("disable"),
            "{err}"
        );
    }

    #[test]
    fn disable_after_prior_ack_ok() {
        let before = AgentSessionPolicy {
            acknowledge_unsafe: true,
            ..Default::default()
        };
        let prop = build_policy_proposal(
            before,
            &[("enforcement".into(), "disabled".into())],
            "disable after ack",
        )
        .unwrap();
        assert_eq!(prop.after.enforcement, EnforcementMode::Disabled);
    }

    fn write_test_policy(path: &Path, content: &str) {
        std::fs::write(path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    #[test]
    fn loads_legacy_degen_policy_toml_when_sentient_missing() {
        let dir = tempdir().unwrap();
        let legacy = dir.path().join(LEGACY_DEGEN_POLICY_TOML);
        write_test_policy(&legacy, "max_slippage_bps = 250\n");
        let loaded = load_policy(dir.path()).unwrap();
        assert_eq!(loaded.max_slippage_bps, 250);
    }

    #[test]
    fn sentient_policy_takes_precedence_over_legacy_degen_policy() {
        let dir = tempdir().unwrap();
        write_test_policy(
            &dir.path().join(LEGACY_DEGEN_POLICY_TOML),
            "max_slippage_bps = 250\n",
        );
        write_test_policy(
            &dir.path().join(SENTIENT_POLICY_TOML),
            "max_slippage_bps = 100\n",
        );
        assert_eq!(load_policy(dir.path()).unwrap().max_slippage_bps, 100);
    }

    #[test]
    fn ack_alone_ok() {
        let before = AgentSessionPolicy::default();
        let prop = build_policy_proposal(
            before,
            &[("acknowledge_unsafe".into(), "true".into())],
            "ack only",
        )
        .unwrap();
        assert!(prop.after.acknowledge_unsafe);
        assert_eq!(prop.after.enforcement, EnforcementMode::Enforced);
    }

    #[test]
    fn unknown_field_fails_closed() {
        // A typo'd key must not silently fall back to (weaker) defaults.
        let dir = tempdir().unwrap();
        write_test_policy(
            &dir.path().join(SENTIENT_POLICY_TOML),
            "max_slippage_bpss = 500\n",
        );
        assert!(load_policy(dir.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn saved_policy_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        save_policy(dir.path(), &AgentSessionPolicy::default()).unwrap();
        let mode = std::fs::metadata(dir.path().join(SENTIENT_POLICY_TOML))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn test_policy_file_mode_0644_refused_mode_0600_accepted() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join(SENTIENT_POLICY_TOML);
        std::fs::write(&path, "max_slippage_bps = 100\n").unwrap();

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = load_policy(dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("0644") && msg.contains("0600"),
            "expected mode error mentioning 0644 and 0600, got: {msg}"
        );

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let loaded = load_policy(dir.path()).unwrap();
        assert_eq!(loaded.max_slippage_bps, 100);
    }

    #[test]
    fn policy_defaults_and_validation() {
        let p = AgentSessionPolicy::default();
        assert_eq!(p.max_position_pct, 10);
        assert_eq!(p.daily_cap_pct, 20);
        assert!(p.allow_transfer_to.is_empty());
        assert!(p.validate().is_ok());

        let mut bad_cap = p.clone();
        bad_cap.daily_cap_pct = 0;
        assert!(bad_cap.validate().is_err());

        let mut bad_cap2 = p.clone();
        bad_cap2.daily_cap_pct = 101;
        assert!(bad_cap2.validate().is_err());

        let mut bad_addr = p.clone();
        bad_addr.allow_transfer_to = vec!["not-an-address".into()];
        assert!(bad_addr.validate().is_err());

        let mut duplicate_addr = p.clone();
        duplicate_addr.allow_transfer_to = vec![
            "0x0000000000000000000000000000000000000001".into(),
            "0x0000000000000000000000000000000000000001".into(),
        ];
        assert!(duplicate_addr.validate().is_err());

        // Dedupe case-insensitively
        let mut case_dupe = p;
        case_dupe.allow_transfer_to = vec![
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".into(),
            "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".into(),
        ];
        assert!(case_dupe.validate().is_err());
    }

    #[test]
    fn policy_file_with_bad_address_fails_closed() {
        let dir = tempdir().unwrap();
        write_test_policy(
            &dir.path().join(SENTIENT_POLICY_TOML),
            "allow_transfer_to = [\"0xinvalid\"]\n",
        );
        assert!(load_policy(dir.path()).is_err());
    }
}
