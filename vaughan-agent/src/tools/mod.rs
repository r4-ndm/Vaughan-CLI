//! Structured Sensory and Proposal Tool Engine.

pub mod execute_degen_swap;
pub mod get_balance;
pub mod get_dex_reserves;
pub mod inspect_contract;
pub mod proposals;
pub mod registry;
pub mod search_pairs;
pub mod simulate_call;

pub use execute_degen_swap::ExecuteDegenSwapTool;
pub use get_balance::GetBalanceTool;
pub use get_dex_reserves::GetDexReservesTool;
pub use inspect_contract::InspectContractTool;
pub use proposals::{
    ProposeBatch7702Tool, ProposeContractCallTool, ProposeSwapTool, ProposeTransferTool,
};
pub use registry::ToolRegistry;
pub use search_pairs::SearchPairsTool;
pub use simulate_call::SimulateCallTool;

use alloy::primitives::Address;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::degen::DegenTrader;
use crate::error::AgentError;
use crate::types::ToolDefinition;

/// Execution context provided to each tool invocation.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub rpc_url: String,
    pub chain_id: u64,
    pub active_address: Option<Address>,
}

/// Construct a default [`ToolRegistry`] populated with all read-only sensory tools.
pub fn default_sensory_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(InspectContractTool::new()));
    registry.register(Arc::new(GetBalanceTool::new()));
    registry.register(Arc::new(GetDexReservesTool::new()));
    registry.register(Arc::new(SearchPairsTool::new()));
    registry.register(Arc::new(SimulateCallTool::new()));
    registry
}

/// Construct a full [`ToolRegistry`] for AI Assisted Mode (sensory + write proposal tools).
pub fn default_assist_registry() -> ToolRegistry {
    let mut registry = default_sensory_registry();
    registry.register(Arc::new(ProposeTransferTool::new()));
    registry.register(Arc::new(ProposeSwapTool::new()));
    registry.register(Arc::new(ProposeBatch7702Tool::new()));
    registry.register(Arc::new(ProposeContractCallTool::new()));
    registry
}

/// Degen Bot registry: sensory tools + autonomous `execute_degen_swap` (no propose-only path).
pub fn default_degen_registry(trader: Arc<DegenTrader>) -> ToolRegistry {
    let mut registry = default_sensory_registry();
    registry.register(Arc::new(ExecuteDegenSwapTool::new(trader)));
    registry
}

/// Trait implemented by all agent tools.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool function name matched by LLM function calls.
    fn name(&self) -> &str;

    /// Human-readable purpose and capability description for the LLM prompt.
    fn description(&self) -> &str;

    /// JSON Schema describing accepted arguments.
    fn parameters(&self) -> Value;

    /// Generates the standard [`ToolDefinition`].
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters(),
        }
    }

    /// Execute the tool given validated arguments and context.
    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError>;
}
