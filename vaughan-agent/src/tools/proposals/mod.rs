//! Write Proposal Tools for AI Assisted Mode.

mod fee;

pub mod propose_agg_swap;
pub mod propose_approve;
pub mod propose_batch_7702;
pub mod propose_contract_call;
pub mod propose_revoke;
pub mod propose_stealth_send;
pub mod propose_swap;
pub mod propose_token_launch;
pub mod propose_transfer;
pub mod propose_v3_lp_lifecycle;
pub mod propose_v3_mint;
pub mod propose_v3_swap;
pub mod propose_wrap;

pub use propose_agg_swap::ProposeAggSwapTool;
pub use propose_approve::ProposeApproveTool;
pub use propose_batch_7702::ProposeBatch7702Tool;
pub use propose_contract_call::ProposeContractCallTool;
pub use propose_revoke::ProposeRevokeTool;
pub use propose_stealth_send::ProposeStealthSendTool;
pub use propose_swap::ProposeSwapTool;
pub use propose_token_launch::ProposeTokenLaunchTool;
pub use propose_transfer::ProposeTransferTool;
pub use propose_v3_lp_lifecycle::{
    ProposeV3CollectTool, ProposeV3DecreaseTool, ProposeV3IncreaseTool,
};
pub use propose_v3_mint::ProposeV3MintTool;
pub use propose_v3_swap::ProposeV3SwapTool;
pub use propose_wrap::{ProposeUnwrapTool, ProposeWrapTool};

pub use fee::attach_estimated_fee;
