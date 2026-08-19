//! Transaction proposal definitions for human review in AI Assisted Mode.

use alloy::primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};

/// Type of proposed on-chain action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProposalType {
    NativeTransfer {
        to: Address,
        amount_wei: U256,
    },
    Erc20Transfer {
        token: Address,
        recipient: Address,
        amount: U256,
    },
    DexSwap {
        router: Address,
        path: Vec<Address>,
        amount_in: U256,
        min_amount_out: U256,
    },
    Batch7702 {
        target_count: usize,
        total_value: U256,
    },
    ContractCall {
        target: Address,
        function_name: Option<String>,
    },
}

/// A structured transaction proposal prepared by the AI Advisor.
///
/// Under the strict security boundary:
/// 1. The advisor cannot sign or broadcast this directly.
/// 2. The proposal is handed to the wallet UI where raw calldata is decoded
///    and simulated independently of LLM descriptions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxProposal {
    pub proposal_id: String,
    pub proposal_type: ProposalType,
    pub to: Address,
    pub value_wei: U256,
    pub calldata: Bytes,
    pub gas_limit: u64,
    pub simulation_success: bool,
    pub estimated_fee_wei: Option<U256>,
    pub llm_explanation: String,
}

impl TxProposal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        proposal_id: impl Into<String>,
        proposal_type: ProposalType,
        to: Address,
        value_wei: U256,
        calldata: Bytes,
        gas_limit: u64,
        simulation_success: bool,
        llm_explanation: impl Into<String>,
    ) -> Self {
        Self {
            proposal_id: proposal_id.into(),
            proposal_type,
            to,
            value_wei,
            calldata,
            gas_limit,
            simulation_success,
            estimated_fee_wei: None,
            llm_explanation: llm_explanation.into(),
        }
    }
}
