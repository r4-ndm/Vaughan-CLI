//! One-off helper: enqueue an LP Brew step from a persisted job (dev only).
//!
//! ```sh
//! cargo test -p vaughan-core --test lp_requeue_manual -- --nocapture
//! ```

use std::path::PathBuf;

use vaughan_core::core::proposal::ProposalQueue;
use vaughan_core::core::{
    lp_deploy_estimate_gas_limit, lp_deploy_job_load, lp_deploy_step_to_proposal,
    v3_lp_prepare_deploy_step, McpSessionToken, V3LpDeployParams,
};

#[tokio::test]
#[ignore = "manual dev helper requiring local profile and unlocked session"]
async fn requeue_latest_lp_brew_job() {
    let prof = PathBuf::from("/home/r4/.local/share/vaughan-cli");
    let job_id = "lp_93639171";
    let job = lp_deploy_job_load(&prof, job_id).expect("load job");
    let params = V3LpDeployParams::try_from(&job.params).expect("params");
    let (tx, label) = v3_lp_prepare_deploy_step(&params)
        .await
        .expect("prepare step");
    eprintln!("step: {label}");
    let gas = lp_deploy_estimate_gas_limit(&params.rpc_url, params.chain_id, &tx)
        .await
        .expect("gas estimate");
    let proposal =
        lp_deploy_step_to_proposal(&job, &label, tx, gas, true, None).expect("proposal");
    let session = McpSessionToken::read(&prof)
        .expect("read session")
        .expect("session missing — unlock TUI");
    ProposalQueue::new(&prof)
        .enqueue(proposal.clone(), "cursor-fix", session.as_bytes())
        .expect("enqueue");
    eprintln!("queued {}", proposal.proposal_id);
}
