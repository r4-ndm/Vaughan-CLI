//! LP Brew CLI: dry-run plan and deploy via MCP proposal queue.

use std::path::{Path, PathBuf};

use clap::Args;
use vaughan_agent::paths::profile_dir;
use vaughan_core::core::proposal::ProposalQueue;
use vaughan_core::core::{
    load_brew_file, lp_deploy_estimate_gas_limit, lp_deploy_job_create, lp_deploy_next_step,
    lp_deploy_plan, lp_deploy_preflight, lp_deploy_step_to_proposal,
    lp_human_inputs_to_deploy_params, LpDeployStepOutcome, LpHumanInputs, LpRangeInput,
    WalletState,
};
use vaughan_core::error::WalletError;

#[derive(Debug, Args)]
pub struct LpPlanArgs {
    #[command(flatten)]
    pub brew: LpBrewArgs,
}

#[derive(Debug, Args)]
pub struct LpDeployArgs {
    #[command(flatten)]
    pub brew: LpBrewArgs,
}

#[derive(Debug, Args)]
pub struct LpBrewArgs {
    /// Load a user Brew JSON file (`--brew /path/to/brew.json`).
    #[arg(long)]
    pub brew: Option<PathBuf>,
    #[arg(long)]
    pub token_a: Option<String>,
    #[arg(long)]
    pub token_b: Option<String>,
    #[arg(long)]
    pub price: Option<String>,
    #[arg(long)]
    pub deposit: Option<String>,
    #[arg(long)]
    pub deposit_token: Option<String>,
    #[arg(long)]
    pub fee: Option<u32>,
    #[arg(long, default_value = "full")]
    pub range: String,
    #[arg(long)]
    pub network: Option<String>,
    #[arg(long)]
    pub rpc_url: Option<String>,
}

pub async fn run_lp_plan(wallet: &WalletState, args: LpPlanArgs) -> Result<(), WalletError> {
    let params = brew_to_params(wallet, &args.brew).await?;
    let plan = lp_deploy_plan(&params).await?;
    println!("Lifecycle: {:?}", plan.lifecycle);
    println!("Steps:");
    for (i, s) in plan.steps.iter().enumerate() {
        println!("  {}. {s}", i + 1);
    }
    Ok(())
}

pub async fn run_lp_deploy(
    wallet: &WalletState,
    profile_path: &Path,
    args: LpDeployArgs,
) -> Result<(), WalletError> {
    let params = brew_to_params(wallet, &args.brew).await?;
    lp_deploy_preflight(&params).await?;

    let prof = profile_dir(profile_path);
    let job_id = format!("cli_lp_{}", uuid_simple());
    let mut job = lp_deploy_job_create(&prof, &job_id, &params, "CLI LP Brew deploy")?;
    let outcome = lp_deploy_next_step(&mut job).await?;
    let LpDeployStepOutcome::Step { tx, label, .. } = outcome else {
        return Err(WalletError::Other("nothing to deploy".into()));
    };
    vaughan_core::core::lp_deploy_job_save(&prof, &job)?;

    let gas = lp_deploy_estimate_gas_limit(&params.rpc_url, params.chain_id, &tx).await?;
    let proposal = lp_deploy_step_to_proposal(&job, &label, (*tx).clone(), gas, true, None)?;
    let queue = ProposalQueue::new(&prof);
    let session = vaughan_core::core::McpSessionToken::read(&prof)
        .ok()
        .flatten()
        .unwrap_or_default();
    if session.is_empty() {
        return Err(WalletError::Other(
            "unlock Vaughan TUI (Advisor) so MCP session exists — CLI enqueues proposals only"
                .into(),
        ));
    }
    queue
        .enqueue(proposal, "cli", session.as_bytes())
        .map_err(WalletError::from)?;
    println!("Queued LP Brew step 1 ({label}) — approve in Vaughan TUI (job {job_id})");
    Ok(())
}

async fn brew_to_params(
    wallet: &WalletState,
    args: &LpBrewArgs,
) -> Result<vaughan_core::core::V3LpDeployParams, WalletError> {
    if let Some(path) = &args.brew {
        let file = load_brew_file(path)?;
        let inputs = human_from_file(wallet, file)?;
        return lp_human_inputs_to_deploy_params(&inputs).await;
    }
    let inputs = LpHumanInputs {
        from: wallet.active_address()?.to_string(),
        chain_id: wallet.networks().active().chain_id,
        rpc_url: args
            .rpc_url
            .clone()
            .unwrap_or_else(|| wallet.active_rpc_url()),
        venue: None,
        token_a: args
            .token_a
            .clone()
            .ok_or_else(|| WalletError::InvalidTransaction("missing --token-a".into()))?,
        token_b: args
            .token_b
            .clone()
            .ok_or_else(|| WalletError::InvalidTransaction("missing --token-b".into()))?,
        price: args
            .price
            .clone()
            .ok_or_else(|| WalletError::InvalidTransaction("missing --price".into()))?,
        deposit: args
            .deposit
            .clone()
            .ok_or_else(|| WalletError::InvalidTransaction("missing --deposit".into()))?,
        deposit_token: args
            .deposit_token
            .clone()
            .ok_or_else(|| WalletError::InvalidTransaction("missing --deposit-token".into()))?,
        fee: args.fee,
        range: if args.range == "full" {
            LpRangeInput::Full
        } else {
            return Err(WalletError::InvalidTransaction(
                "only --range full supported".into(),
            ));
        },
    };
    lp_human_inputs_to_deploy_params(&inputs).await
}

fn human_from_file(
    wallet: &WalletState,
    file: vaughan_core::core::LpDeployBrewFile,
) -> Result<LpHumanInputs, WalletError> {
    Ok(LpHumanInputs {
        from: wallet.active_address()?.to_string(),
        chain_id: wallet.networks().active().chain_id,
        rpc_url: wallet.active_rpc_url(),
        venue: file.venue,
        token_a: file.token_a,
        token_b: file.token_b,
        price: file.price,
        deposit: file.deposit,
        deposit_token: file.deposit_token,
        fee: file.fee,
        range: file.range,
    })
}

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        .to_string()
}
