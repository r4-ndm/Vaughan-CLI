//! Degen `/policy` commands and approval-card apply.

use vaughan_agent::{
    build_system_prompt, load_policy, save_policy, AgentSessionPolicy, EnforcementMode,
    DEGEN_POLICY_TOML,
};
use vaughan_core::core::profile::OperatingMode;

use super::{AgentMessage, AgentView};
use crate::app::KeyOutcome;

impl AgentView {
    /// `/policy` — show / reload / set Degen guardrails (`degen-policy.toml`).
    pub(super) fn handle_policy_command(&mut self, rest: &str) -> KeyOutcome {
        if self.operating_mode != OperatingMode::DegenTrader {
            self.history.push(AgentMessage::System(
                "/policy is for Degen Bot mode (burner profile). Switch mode in Settings.".into(),
            ));
            return KeyOutcome::Consumed;
        }
        let Some(dir) = self.profile_dir.clone() else {
            self.history.push(AgentMessage::System(
                "No profile directory — cannot load degen-policy.toml.".into(),
            ));
            return KeyOutcome::Consumed;
        };

        let tokens: Vec<&str> = rest.split_whitespace().collect();
        if tokens.is_empty() {
            let policy = load_policy(&dir).unwrap_or_default();
            let mut lines = vec![format!("Degen policy (`{DEGEN_POLICY_TOML}`):")];
            lines.extend(policy.summary_lines());
            lines.push(
                "Commands: /policy reload | /policy confirm-unsafe | /policy set <key> <value>"
                    .into(),
            );
            lines.push(
                "Keys: enforcement | max_position_pct | max_slippage_bps | max_session_gas_wei | \
                 max_consecutive_errors | required_rpc_quorum"
                    .into(),
            );
            self.history.push(AgentMessage::System(lines.join("\n")));
            return KeyOutcome::Consumed;
        }

        match tokens[0].to_ascii_lowercase().as_str() {
            "reload" => match load_policy(&dir) {
                Ok(policy) => match self.apply_policy_to_session(policy) {
                    Ok(msg) => self.history.push(AgentMessage::System(msg)),
                    Err(e) => self.history.push(AgentMessage::System(e)),
                },
                Err(e) => self
                    .history
                    .push(AgentMessage::System(format!("reload failed: {e}"))),
            },
            "confirm-unsafe" => {
                let mut policy = load_policy(&dir).unwrap_or_default();
                policy.acknowledge_unsafe = true;
                match save_policy(&dir, &policy) {
                    Ok(()) => self.history.push(AgentMessage::System(
                        "acknowledge_unsafe = true saved. You may now \
                         `/policy set enforcement disabled` (Esc still stops trading)."
                            .into(),
                    )),
                    Err(e) => self
                        .history
                        .push(AgentMessage::System(format!("save failed: {e}"))),
                }
            }
            "set" if tokens.len() >= 3 => {
                let key = tokens[1].to_ascii_lowercase();
                let value = tokens[2..].join(" ");
                let mut policy = load_policy(&dir).unwrap_or_default();
                if let Err(e) = policy.set_field(&key, &value) {
                    self.history.push(AgentMessage::System(e.to_string()));
                    return KeyOutcome::Consumed;
                }
                match save_policy(&dir, &policy) {
                    Ok(()) => match self.apply_policy_to_session(policy) {
                        Ok(msg) => self.history.push(AgentMessage::System(msg)),
                        Err(e) => self.history.push(AgentMessage::System(e)),
                    },
                    Err(e) => self
                        .history
                        .push(AgentMessage::System(format!("save failed: {e}"))),
                }
            }
            _ => self.history.push(AgentMessage::System(
                "Usage: /policy | /policy reload | /policy confirm-unsafe | /policy set <key> <value>"
                    .into(),
            )),
        }
        KeyOutcome::Consumed
    }

    fn apply_policy_to_session(&mut self, policy: AgentSessionPolicy) -> Result<String, String> {
        let cfg = policy.to_breaker_config().map_err(|e| e.to_string())?;
        if let Some(ref trader) = self.degen {
            trader.apply_breaker_config(cfg.clone());
        }
        self.session.max_position_pct = Some(cfg.max_position_pct);
        self.session.max_slippage_bps = Some(cfg.max_slippage_bps);
        // Refresh system prompt so the model sees new limits.
        if let Some(sys) = self.llm_history.first_mut() {
            *sys = build_system_prompt(
                self.operating_mode,
                self.profile_dir.as_deref(),
                Some(&self.session),
            );
        }
        let banner = if cfg.enforcement == EnforcementMode::Disabled {
            "⚠ Breakers DISABLED for testing — Esc still emergency-stops."
        } else {
            "Breakers updated for this session."
        };
        Ok(format!("{banner}\n{}", policy.summary_lines().join("\n")))
    }

    pub(super) fn accept_policy_proposal(&mut self) {
        let Some(prop) = self.active_policy_proposal.take() else {
            return;
        };
        let Some(dir) = self.profile_dir.clone() else {
            self.history.push(AgentMessage::System(
                "No profile directory — cannot save policy.".into(),
            ));
            return;
        };
        match vaughan_agent::commit_policy_proposal(&dir, &prop) {
            Ok(policy) => match self.apply_policy_to_session(policy) {
                Ok(msg) => {
                    self.status = format!("Policy {} applied.", prop.proposal_id);
                    self.history.push(AgentMessage::System(format!(
                        "Approved {}.\n{msg}",
                        prop.proposal_id
                    )));
                }
                Err(e) => {
                    self.history
                        .push(AgentMessage::System(format!("Saved but apply failed: {e}")));
                }
            },
            Err(e) => {
                self.history
                    .push(AgentMessage::System(format!("Could not save policy: {e}")));
            }
        }
    }
}
