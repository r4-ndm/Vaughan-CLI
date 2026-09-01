//! Proposal tool: full V3 LP deploy Brew → multi-step job or single EIP-7702 batch.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::str::FromStr;
use url::Url;

use crate::error::AgentError;
use crate::proposal::{ProposalType, TxProposal};
use crate::tools::proposals::attach_estimated_fee;
use crate::tools::proposals::propose_transfer::rand_id;
use crate::tools::v3_lp::{proposal_network_id, resolve_lp_venue, venue_param_schema};
use crate::tools::{Tool, ToolContext};
use vaughan_aa::abi::Transaction;
use vaughan_aa::encode::encode_execute;
use vaughan_aa::{estimate_self_pay_fee, ScwTransaction, DEFAULT_BATCH_GAS_LIMIT};
use vaughan_core::chains::evm::networks::get_network_by_chain_id;
use vaughan_core::chains::evm::EvmAdapter;
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::{
    build_lp_deploy_batch_calls, lp_deploy_estimate_gas_limit, lp_deploy_job_create,
    lp_deploy_next_step, lp_deploy_preflight, lp_deploy_step_to_proposal,
    lp_human_inputs_to_deploy_params, LpDeployStepOutcome, LpHumanInputs, LpRangeInput,
};

#[derive(Default)]
pub struct ProposeV3LpDeployTool;

impl ProposeV3LpDeployTool {
    pub fn new() -> Self {
        Self
    }
}

async fn batch_estimated_fee_wei(
    context: &ToolContext,
    account: Address,
    txns: &[Transaction],
) -> Option<U256> {
    let net = get_network_by_chain_id(context.chain_id)?;
    let adapter = EvmAdapter::new(
        &context.rpc_url,
        context.chain_id,
        &net.name,
        &net.fallback_rpc_urls,
    )
    .await
    .ok()?;
    let scw = ScwTransaction {
        account,
        chain_id: context.chain_id,
        nonce: 0,
        txns: txns.to_vec(),
    };
    let placeholder = [0u8; 66];
    let (gas_limit, max_fee, _) = estimate_self_pay_fee(&adapter, &scw, &placeholder, None)
        .await
        .ok()?;
    Some(U256::from(gas_limit).saturating_mul(U256::from(max_fee)))
}

fn evm_to_aa_tx(tx: &EvmTransaction) -> Result<Transaction, AgentError> {
    let to = Address::from_str(tx.to.trim())
        .map_err(|e| AgentError::InvalidToolCall(format!("batch to: {e}")))?;
    let value = U256::from_str_radix(tx.value.trim(), 10)
        .or_else(|_| U256::from_str_radix(tx.value.trim().trim_start_matches("0x"), 16))
        .unwrap_or(U256::ZERO);
    let data = match &tx.data {
        Some(d) if !d.is_empty() => {
            Bytes::from(hex::decode(d.trim_start_matches("0x")).map_err(|e| {
                AgentError::InvalidToolCall(format!("batch calldata: {e}"))
            })?)
        }
        _ => Bytes::new(),
    };
    Ok(Transaction { to, value, data })
}

#[async_trait]
impl Tool for ProposeV3LpDeployTool {
    fn name(&self) -> &str {
        "propose_v3_lp_deploy"
    }

    fn description(&self) -> &str {
        "Start a full V3 LP deploy Brew (create → initialize → approve → mint). \
         Default mode enqueues one step at a time (TUI auto-advances). \
         mode=batch drafts a single EIP-7702 batch. Never signs."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "token_a": { "type": "string", "description": "First token (checksummed 0x address)" },
                "token_b": { "type": "string", "description": "Second token (0x address)" },
                "price": {
                    "type": "string",
                    "description": "Starting price: token_b per token_a (e.g. 0.2)"
                },
                "deposit": { "type": "string", "description": "Human deposit amount" },
                "deposit_token": {
                    "type": "string",
                    "description": "Which token the deposit is in (token_a or token_b symbol)"
                },
                "fee": {
                    "type": "integer",
                    "description": "Fee bps (20000=2%). Omit to discover existing pool tier."
                },
                "range": {
                    "type": "string",
                    "description": "full or omit for full range",
                    "default": "full"
                },
                "mode": {
                    "type": "string",
                    "enum": ["steps", "batch"],
                    "default": "steps",
                    "description": "steps = per-step TUI approve; batch = single propose_batch_7702"
                },
                "venue": venue_param_schema()["venue"],
                "explanation": { "type": "string" }
            },
            "required": ["token_a", "token_b", "price", "deposit", "deposit_token", "explanation"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<Value, AgentError> {
        let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("steps");
        let profile_dir = context.profile_dir.as_ref().ok_or_else(|| {
            AgentError::InvalidToolCall(
                "LP deploy Brew requires profile_dir — use MCP dispatch, not bare registry".into(),
            )
        })?;

        let from = context
            .active_address
            .ok_or_else(|| {
                AgentError::InvalidToolCall(
                    "No active wallet — unlock Vaughan TUI (Advisor mode)".into(),
                )
            })?
            .to_string();

        let explanation = args
            .get("explanation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolCall("Missing explanation".into()))?;

        let venue_label = resolve_lp_venue(&args, context.chain_id)?.label().to_string();
        let range = match args.get("range").and_then(|v| v.as_str()).unwrap_or("full") {
            "full" | "" => LpRangeInput::Full,
            other => {
                return Err(AgentError::InvalidToolCall(format!(
                    "range {other:?} not supported yet — use full"
                )));
            }
        };

        let inputs = LpHumanInputs {
            from,
            chain_id: context.chain_id,
            rpc_url: context.rpc_url.clone(),
            venue: Some(venue_label),
            token_a: args
                .get("token_a")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token_a".into()))?
                .into(),
            token_b: args
                .get("token_b")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing token_b".into()))?
                .into(),
            price: args
                .get("price")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing price".into()))?
                .into(),
            deposit: args
                .get("deposit")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing deposit".into()))?
                .into(),
            deposit_token: args
                .get("deposit_token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AgentError::InvalidToolCall("Missing deposit_token".into()))?
                .into(),
            fee: args.get("fee").and_then(|v| v.as_u64()).map(|f| f as u32),
            range,
        };

        let params = lp_human_inputs_to_deploy_params(&inputs)
            .await
            .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;
        lp_deploy_preflight(&params)
            .await
            .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;

        if mode == "batch" {
            return propose_lp_deploy_batch(&params, explanation, context).await;
        }

        let job_id = format!("lp_{}", rand_id());
        let mut job = lp_deploy_job_create(profile_dir, &job_id, &params, explanation)
            .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;

        let outcome = lp_deploy_next_step(&mut job)
            .await
            .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;

        let LpDeployStepOutcome::Step { tx, label, .. } = outcome else {
            return Err(AgentError::InvalidToolCall(
                "pool already ready with nothing to deploy — use propose_v3_mint".into(),
            ));
        };

        vaughan_core::core::lp_deploy_job_save(profile_dir, &job)
            .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;

        let gas_limit = lp_deploy_estimate_gas_limit(&params.rpc_url, params.chain_id, &tx)
            .await
            .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;
        let mut proposal = lp_deploy_step_to_proposal(&job, &label, (*tx).clone(), gas_limit, true, None)
            .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?
            .with_chain(context.chain_id, proposal_network_id(context));

        proposal = attach_estimated_fee(proposal, context).await;

        Ok(json!({
            "proposal": proposal,
            "job_id": job_id,
            "step": label,
            "message": "First LP Brew step queued — approve in Vaughan TUI; later steps auto-enqueue"
        }))
    }
}

async fn propose_lp_deploy_batch(
    params: &vaughan_core::core::V3LpDeployParams,
    explanation: &str,
    context: &ToolContext,
) -> Result<Value, AgentError> {
    let plan = build_lp_deploy_batch_calls(params)
        .await
        .map_err(|e| AgentError::InvalidToolCall(e.user_message()))?;
    if let Some(warn) = &plan.gas_warning {
        return Err(AgentError::InvalidToolCall(warn.clone()));
    }
    let txns: Vec<Transaction> = plan
        .calls
        .iter()
        .map(evm_to_aa_tx)
        .collect::<Result<Vec<_>, _>>()?;
    let sender = context.active_address.ok_or_else(|| {
        AgentError::InvalidToolCall("No active wallet".into())
    })?;
    let batched_calldata = Bytes::from(encode_execute(&txns, &[0u8; 66]));
    let total_value = txns.iter().fold(U256::ZERO, |acc, t| acc.saturating_add(t.value));

    let rpc_url = Url::parse(&context.rpc_url)
        .map_err(|e| AgentError::InvalidToolCall(format!("Invalid RPC URL: {e}")))?;
    let provider: alloy::providers::RootProvider<alloy::network::Ethereum> =
        alloy::providers::RootProvider::new_http(rpc_url);
    let sim = provider
        .call(
            alloy::rpc::types::eth::TransactionRequest::default()
                .to(sender)
                .input(batched_calldata.clone().into())
                .value(total_value)
                .from(sender),
        )
        .await;
    let sim_success = sim.is_ok();

    let step_list = plan.steps.join(" → ");
    let batch_explanation = format!(
        "{explanation} [LP Brew batch: {step_list}. First batch delegates EOA to AmbireAccount if needed.]"
    );
    let estimated_fee_wei = batch_estimated_fee_wei(context, sender, &txns).await;

    let mut proposal = TxProposal::new(
        format!("lp_batch_{}", rand_id()),
        ProposalType::Batch7702 {
            target_count: txns.len(),
            total_value,
        },
        sender,
        total_value,
        batched_calldata,
        DEFAULT_BATCH_GAS_LIMIT,
        sim_success,
        batch_explanation,
    )
    .with_chain(context.chain_id, proposal_network_id(context));
    proposal.estimated_fee_wei = estimated_fee_wei;

    Ok(json!({
        "proposal": proposal,
        "mode": "batch",
        "steps": plan.steps,
        "message": "LP Brew batch queued — one TUI approve runs create→init→approve→mint atomically"
    }))
}
