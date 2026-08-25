//! Degen session policy — user-owned circuit-breaker dials.
//!
//! Persisted as `degen-policy.toml` beside the profile wallet. Agents may
//! **propose** changes; only the human (via `/policy` or a future approval
//! card) writes the file. Safe defaults match [`CircuitBreakerConfig::default`].

use std::fs;
use std::path::Path;
use std::str::FromStr;

use alloy::primitives::U256;
use serde::{Deserialize, Serialize};

use super::circuit_breaker::CircuitBreakerConfig;
use crate::error::AgentError;

/// Filename next to `wallet.json` / `agent.toml`.
pub const DEGEN_POLICY_TOML: &str = "degen-policy.toml";

/// How hard the Rust breakers enforce limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EnforcementMode {
    /// Reject oversize / overslippage; trip on gas / consecutive errors (default).
    #[default]
    Enforced,
    /// Allow trades that would fail under Enforced; log a warning (lab / tuning).
    WarnOnly,
    /// Skip position / slippage / gas / error tripwires. Esc emergency stop still works.
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

/// User-editable Degen guardrails (burner profile only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    100
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
            max_slippage_bps: default_max_slippage_bps(),
            max_session_gas_wei: default_max_session_gas_wei_str(),
            max_consecutive_errors: default_max_consecutive_errors(),
            required_rpc_quorum: default_required_rpc_quorum(),
        }
    }
}

impl AgentSessionPolicy {
    /// Validate ranges and unsafe acknowledgement.
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.max_position_pct == 0 || self.max_position_pct > 100 {
            return Err(AgentError::InvalidToolCall(
                "max_position_pct must be 1..=100".into(),
            ));
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

    /// Human-readable summary for `/policy` and system banners.
    pub fn summary_lines(&self) -> Vec<String> {
        let unsafe_note = if self.enforcement == EnforcementMode::Disabled {
            " ⚠ UNSAFE — breakers off (Esc still stops)"
        } else {
            ""
        };
        vec![
            format!("enforcement: {}{}", self.enforcement.as_str(), unsafe_note),
            format!("max_position_pct: {}%", self.max_position_pct),
            format!(
                "max_slippage_bps: {} ({}%)",
                self.max_slippage_bps,
                self.max_slippage_bps as f64 / 100.0
            ),
            format!("max_session_gas_wei: {}", self.max_session_gas_wei),
            format!("max_consecutive_errors: {}", self.max_consecutive_errors),
            format!("required_rpc_quorum: {}", self.required_rpc_quorum),
            format!("acknowledge_unsafe: {}", self.acknowledge_unsafe),
        ]
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

impl PolicyProposal {
    pub fn summary_card(&self) -> Vec<String> {
        let mut lines = vec![
            format!("Policy proposal {}", self.proposal_id),
            self.llm_explanation.clone(),
            String::new(),
            "Changes:".into(),
        ];
        lines.extend(self.changes.iter().cloned());
        if self.after.enforcement == EnforcementMode::Disabled {
            lines.push(String::new());
            lines.push("⚠ Would DISABLE breakers (Esc still emergency-stops).".into());
        }
        lines
    }
}

/// Build a [`PolicyProposal`] from a list of key/value patches on `before`.
pub fn build_policy_proposal(
    before: AgentSessionPolicy,
    patches: &[(String, String)],
    explanation: impl Into<String>,
) -> Result<PolicyProposal, AgentError> {
    if patches.is_empty() {
        return Err(AgentError::InvalidToolCall(
            "propose_policy needs at least one change".into(),
        ));
    }
    let explanation = explanation.into();
    let mut after = before.clone();
    let mut changes = Vec::new();
    for (key, value) in patches {
        let old = match key.as_str() {
            "enforcement" => before.enforcement.as_str().to_string(),
            "max_position_pct" => before.max_position_pct.to_string(),
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

/// Load policy from `dir/degen-policy.toml`, or defaults if missing.
pub fn load_policy(dir: &Path) -> Result<AgentSessionPolicy, AgentError> {
    let path = dir.join(DEGEN_POLICY_TOML);
    if !path.exists() {
        return Ok(AgentSessionPolicy::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| {
        AgentError::ProviderError(format!("failed to read {}: {e}", path.display()))
    })?;
    let policy: AgentSessionPolicy = toml::from_str(&raw)
        .map_err(|e| AgentError::ProviderError(format!("invalid {DEGEN_POLICY_TOML}: {e}")))?;
    policy.validate()?;
    Ok(policy)
}

/// Atomically write `degen-policy.toml` after validation.
pub fn save_policy(dir: &Path, policy: &AgentSessionPolicy) -> Result<(), AgentError> {
    policy.validate()?;
    fs::create_dir_all(dir).map_err(|e| {
        AgentError::ProviderError(format!("failed to create {}: {e}", dir.display()))
    })?;
    let path = dir.join(DEGEN_POLICY_TOML);
    let tmp = dir.join(format!(".{DEGEN_POLICY_TOML}.tmp"));
    let body = toml::to_string_pretty(policy)
        .map_err(|e| AgentError::ProviderError(format!("serialize policy: {e}")))?;
    fs::write(&tmp, body).map_err(|e| {
        AgentError::ProviderError(format!("failed to write {}: {e}", tmp.display()))
    })?;
    fs::rename(&tmp, &path).map_err(|e| {
        AgentError::ProviderError(format!("failed to replace {}: {e}", path.display()))
    })?;
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
    // Cannot require more RPCs than configured endpoints.
    policy.required_rpc_quorum = policy.required_rpc_quorum.min(rpc_count.max(1));
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
}
