//! Tool registry for dispatching tool calls from LLM responses.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use crate::error::AgentError;
use crate::tools::{Tool, ToolContext};
use crate::types::ToolDefinition;

/// Registry holding all available tools.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a new tool.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Retrieve all tool definitions formatted for LLM schema declarations.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    /// Dispatch and execute a tool call by name.
    pub async fn execute(
        &self,
        name: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<Value, AgentError> {
        let tool = self.tools.get(name).ok_or_else(|| {
            AgentError::InvalidToolCall(format!("Tool '{name}' is not registered"))
        })?;

        tool.execute(args, context).await
    }
}
