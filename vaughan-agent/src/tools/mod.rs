//! Structured Sensory and Proposal Tool Engine.

pub mod execute_degen_swap;
pub mod get_balance;
pub mod get_dex_reserves;
pub mod get_v3_pool;
pub mod inspect_contract;
pub mod list_allowances;
pub mod list_transfers;
pub mod list_v3_positions;
pub mod proposals;
pub mod propose_policy;
pub mod quote_bridge;
pub mod quote_swap;
pub mod quote_v3_swap;
pub mod registry;
pub mod resolve_token;
pub mod search_pairs;
pub mod simulate_call;
pub mod watch_balance;
pub mod wiz4rd_common;

pub use execute_degen_swap::ExecuteDegenSwapTool;
pub use get_balance::GetBalanceTool;
pub use get_dex_reserves::GetDexReservesTool;
pub use get_v3_pool::GetV3PoolTool;
pub use inspect_contract::InspectContractTool;
pub use list_allowances::ListAllowancesTool;
pub use list_transfers::ListTransfersTool;
pub use list_v3_positions::ListV3PositionsTool;
pub use proposals::{
    ProposeAggSwapTool, ProposeApproveTool, ProposeBatch7702Tool, ProposeContractCallTool,
    ProposeRevokeTool, ProposeStealthSendTool, ProposeSwapTool, ProposeTransferTool,
    ProposeUnwrapTool, ProposeV3CollectTool, ProposeV3DecreaseTool, ProposeV3IncreaseTool,
    ProposeV3MintTool, ProposeV3SwapTool, ProposeWrapTool,
};
pub use propose_policy::{commit_policy_proposal, ProposePolicyTool};
pub use quote_bridge::{ProposeBridgeTool, QuoteBridgeTool};
pub use quote_swap::QuoteSwapTool;
pub use quote_v3_swap::QuoteV3SwapTool;
pub use registry::ToolRegistry;
pub use resolve_token::{ImportTokenTool, ResolveTokenTool};
pub use search_pairs::SearchPairsTool;
pub use simulate_call::SimulateCallTool;
pub use watch_balance::WatchBalanceTool;

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
    registry.register(Arc::new(ListTransfersTool::new()));
    registry.register(Arc::new(ResolveTokenTool::new()));
    registry.register(Arc::new(QuoteBridgeTool::new()));
    registry.register(Arc::new(WatchBalanceTool::new()));
    registry
}

/// Assist registry; pass `profile_dir` to enable `import_token`.
pub fn default_assist_registry() -> ToolRegistry {
    default_assist_registry_for(None)
}

/// Construct a full assist registry (sensory + write proposals).
pub fn default_assist_registry_for(profile_dir: Option<&Path>) -> ToolRegistry {
    let mut registry = default_sensory_registry();
    registry.register(Arc::new(ProposeTransferTool::new()));
    registry.register(Arc::new(ProposeSwapTool::new()));
    registry.register(Arc::new(ProposeAggSwapTool::new()));
    registry.register(Arc::new(ProposeV3SwapTool::new()));
    registry.register(Arc::new(ProposeV3MintTool::new()));
    registry.register(Arc::new(ProposeV3IncreaseTool::new()));
    registry.register(Arc::new(ProposeV3DecreaseTool::new()));
    registry.register(Arc::new(ProposeV3CollectTool::new()));
    registry.register(Arc::new(ProposeWrapTool::new()));
    registry.register(Arc::new(ProposeUnwrapTool::new()));
    registry.register(Arc::new(ProposeApproveTool::new()));
    registry.register(Arc::new(ProposeRevokeTool::new()));
    registry.register(Arc::new(ProposeBridgeTool::new()));
    registry.register(Arc::new(ProposeStealthSendTool::new()));
    registry.register(Arc::new(ProposeBatch7702Tool::new()));
    registry.register(Arc::new(ProposeContractCallTool::new()));
    if let Some(dir) = profile_dir {
        registry.register(Arc::new(ImportTokenTool::new(dir.to_path_buf())));
    }
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
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters(),
        }
    }
    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError>;
}
