//! Structured Sensory and Proposal Tool Engine.

pub mod execute_degen_swap;
pub mod get_balance;
pub mod get_dex_reserves;
pub mod get_v3_pool;
pub mod inspect_contract;
pub mod list_allowances;
pub mod list_v3_positions;
pub mod proposals;
pub mod propose_policy;
pub mod quote_swap;
pub mod quote_v3_swap;
pub mod registry;
pub mod search_pairs;
pub mod simulate_call;
pub mod wiz4rd_common;

pub use execute_degen_swap::ExecuteDegenSwapTool;
pub use get_balance::GetBalanceTool;
pub use get_dex_reserves::GetDexReservesTool;
pub use get_v3_pool::GetV3PoolTool;
pub use inspect_contract::InspectContractTool;
pub use list_allowances::ListAllowancesTool;
pub use list_v3_positions::ListV3PositionsTool;
pub use proposals::{
    ProposeAggSwapTool, ProposeBatch7702Tool, ProposeContractCallTool, ProposeRevokeTool,
    ProposeSwapTool, ProposeTransferTool, ProposeUnwrapTool, ProposeV3MintTool, ProposeV3SwapTool,
    ProposeWrapTool,
};
pub use propose_policy::{commit_policy_proposal, ProposePolicyTool};
pub use quote_swap::QuoteSwapTool;
pub use quote_v3_swap::QuoteV3SwapTool;
pub use registry::ToolRegistry;
pub use search_pairs::SearchPairsTool;
pub use simulate_call::SimulateCallTool;

use alloy::primitives::Address;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
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
    registry.register(Arc::new(QuoteSwapTool::new()));
    registry.register(Arc::new(GetV3PoolTool::new()));
    registry.register(Arc::new(QuoteV3SwapTool::new()));
    registry.register(Arc::new(ListAllowancesTool::new()));
    registry.register(Arc::new(ListV3PositionsTool::new()));
    registry
}

/// Construct a full [`ToolRegistry`] for AI Assisted Mode (sensory + write proposal tools).
pub fn default_assist_registry() -> ToolRegistry {
    let mut registry = default_sensory_registry();
    registry.register(Arc::new(ProposeTransferTool::new()));
    registry.register(Arc::new(ProposeSwapTool::new()));
    registry.register(Arc::new(ProposeAggSwapTool::new()));
    registry.register(Arc::new(ProposeV3SwapTool::new()));
    registry.register(Arc::new(ProposeV3MintTool::new()));
    registry.register(Arc::new(ProposeWrapTool::new()));
    registry.register(Arc::new(ProposeUnwrapTool::new()));
    registry.register(Arc::new(ProposeRevokeTool::new()));
    registry.register(Arc::new(ProposeBatch7702Tool::new()));
    registry.register(Arc::new(ProposeContractCallTool::new()));
    registry
}

/// Degen Bot registry: sensory + `execute_degen_swap` + `propose_policy` (human-approved).
pub fn default_degen_registry(trader: Arc<DegenTrader>, profile_dir: &Path) -> ToolRegistry {
    let mut registry = default_sensory_registry();
    registry.register(Arc::new(ExecuteDegenSwapTool::new(trader)));
    registry.register(Arc::new(ProposePolicyTool::new(profile_dir.to_path_buf())));
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
