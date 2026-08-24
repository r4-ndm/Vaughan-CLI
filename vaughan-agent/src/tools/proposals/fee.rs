//! Attach EIP-1559 fee estimates to agent proposals (same path as wallet TUI).

use alloy::primitives::U256;

use crate::proposal::TxProposal;
use crate::tools::ToolContext;
use vaughan_core::chains::evm::networks::get_network_by_chain_id;
use vaughan_core::chains::evm::EvmAdapter;
use vaughan_core::chains::{ChainAdapter, ChainTransaction};
use vaughan_core::core::TransactionService;

/// Fill [`TxProposal::estimated_fee_wei`] using core [`EvmAdapter::estimate_fee`]
/// (MetaMask/ethers-family EIP-1559 heuristics on Alloy). Returns the proposal
/// unchanged on failure so propose tools never block on RPC fee errors.
pub async fn attach_estimated_fee(mut proposal: TxProposal, context: &ToolContext) -> TxProposal {
    proposal.estimated_fee_wei = estimate_proposal_fee_wei(&proposal, context).await;
    proposal
}

async fn estimate_proposal_fee_wei(proposal: &TxProposal, context: &ToolContext) -> Option<U256> {
    let from = context.active_address?;
    let chain_id = if proposal.chain_id > 0 {
        proposal.chain_id
    } else {
        context.chain_id
    };
    let net = get_network_by_chain_id(chain_id)?;

    let data_hex = if proposal.calldata.is_empty() {
        String::new()
    } else {
        format!("0x{}", hex::encode(&proposal.calldata))
    };

    let svc = TransactionService::new();
    let chain_tx = svc
        .build_contract_call(
            format!("{from:#x}"),
            format!("{:#x}", proposal.to),
            &data_hex,
            proposal.value_wei.to_string(),
            chain_id,
        )
        .ok()?;

    let ChainTransaction::Evm(mut evm) = chain_tx else {
        return None;
    };
    evm.gas_limit = Some(proposal.gas_limit);

    let adapter = EvmAdapter::new(
        &context.rpc_url,
        chain_id,
        &net.name,
        &net.fallback_rpc_urls,
    )
    .await
    .ok()?;

    adapter
        .estimate_fee(&ChainTransaction::Evm(evm))
        .await
        .ok()?
        .total_wei_evm()
}
