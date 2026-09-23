//! Read-only: list persisted DCA plans for the profile.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::dca::load_plans;
use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};

pub struct ListDcaPlansTool {
    profile_dir: Arc<PathBuf>,
}

impl ListDcaPlansTool {
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
impl Tool for ListDcaPlansTool {
    fn name(&self) -> &str {
        "list_dca_plans"
    }

    fn description(&self) -> &str {
        "List recurring-buy (DCA) plans for this profile: status, progress, next_due_at, last slice."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    async fn execute(&self, _args: Value, _context: &ToolContext) -> Result<Value, AgentError> {
        let file = load_plans(self.dir())?;
        serde_json::to_value(&file)
            .map_err(|e| AgentError::ProviderError(format!("serialize dca plans: {e}")))
    }
}
