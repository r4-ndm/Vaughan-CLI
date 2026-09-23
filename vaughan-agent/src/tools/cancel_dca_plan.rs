//! Cancel a DCA plan (stops future slices). Writes disk immediately —
//! agents should only call after the human asked to cancel; TUI also exposes pause/cancel.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::dca::cancel_plan;
use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};

pub struct CancelDcaPlanTool {
    profile_dir: Arc<PathBuf>,
}

impl CancelDcaPlanTool {
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
impl Tool for CancelDcaPlanTool {
    fn name(&self) -> &str {
        "cancel_dca_plan"
    }

    fn description(&self) -> &str {
        "Cancel a DCA plan by id (stops future slices). Prefer after list_dca_plans. \
         Does not reverse past buys."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "plan_id": {
                    "type": "string",
                    "description": "Plan id from list_dca_plans"
                }
            },
            "required": ["plan_id"]
        })
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<Value, AgentError> {
        let id = args
            .get("plan_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing plan_id".into()))?;
        let plan = cancel_plan(self.dir(), id)?;
        Ok(json!({
            "ok": true,
            "plan_id": plan.id,
            "status": plan.status.as_str(),
        }))
    }
}
