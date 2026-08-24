//! Core types for structured tool schemas.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-schema definition of a tool callable by an external agent (MCP).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}
