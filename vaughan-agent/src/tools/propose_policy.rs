//! Propose a Degen session-policy change for human `[a]` / `[d]` approval.
//!
//! Does not sign or broadcast. The TUI applies [`PolicyProposal::after`] only
//! after the user accepts the card.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::degen::policy::{
    build_policy_proposal, load_policy, AgentSessionPolicy, PolicyProposal,
};
use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};

/// Draft a policy patch from the LLM; human must approve before disk/session update.
pub struct ProposePolicyTool {
    profile_dir: Arc<PathBuf>,
}

impl ProposePolicyTool {
    pub fn new(profile_dir: impl Into<PathBuf>) -> Self {
        Self {
            profile_dir: Arc::new(profile_dir.into()),
        }
    }

    fn dir(&self) -> &Path {
        self.profile_dir.as_path()
    }
}

#[async_trait]
impl Tool for ProposePolicyTool {
    fn name(&self) -> &str {
        "propose_policy"
    }

    fn description(&self) -> &str {
        "Propose a Degen guardrail change for the human to approve ([a]/[d] card). \
         Use when the user asks to loosen/tighten breakers or turn them off for testing. \
         Never claim the policy changed until they approve. Keys: enforcement, \
         max_position_pct, max_slippage_bps, max_session_gas_wei, max_consecutive_errors, \
         required_rpc_quorum, acknowledge_unsafe."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "changes": {
                    "type": "object",
                    "description": "Map of policy keys to new string values (e.g. {\"max_slippage_bps\": \"500\"})",
                    "additionalProperties": { "type": "string" }
                },
                "explanation": {
                    "type": "string",
                    "description": "Short reason shown on the approval card"
                }
            },
            "required": ["changes", "explanation"]
        })
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<Value, AgentError> {
        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'explanation'".into()))?
            .to_string();
        let changes_obj = args
            .get("changes")
            .and_then(|v| v.as_object())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing 'changes' object".into()))?;
        let mut patches = Vec::new();
        for (k, v) in changes_obj {
            let val = v
                .as_str()
                .map(str::to_string)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
                .or_else(|| v.as_i64().map(|n| n.to_string()))
                .or_else(|| v.as_bool().map(|b| b.to_string()))
                .ok_or_else(|| {
                    AgentError::InvalidToolCall(format!("changes.{k} must be string/number/bool"))
                })?;
            patches.push((k.clone(), val));
        }
        let before = load_policy(self.dir()).unwrap_or_default();
        let proposal = build_policy_proposal(before, &patches, explanation)?;
        serde_json::to_value(&proposal)
            .map_err(|e| AgentError::ProviderError(format!("serialize policy proposal: {e}")))
    }
}

/// Apply an approved proposal to disk (caller hot-reloads the live breaker).
pub fn commit_policy_proposal(
    dir: &Path,
    proposal: &PolicyProposal,
) -> Result<AgentSessionPolicy, AgentError> {
    save_committed(dir, &proposal.after)?;
    Ok(proposal.after.clone())
}

fn save_committed(dir: &Path, policy: &AgentSessionPolicy) -> Result<(), AgentError> {
    crate::degen::policy::save_policy(dir, policy)
}
