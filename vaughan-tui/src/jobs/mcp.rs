//! Body for the [`crate::jobs::UiJob::McpQueuedApprove`] job, lifted out of
//! `app.rs`'s `spawn_job`.
//!
//! Lock discipline: [`mcp_queued_approve`] holds the wallet lock across the broadcast await
//! (`#[allow(clippy::await_holding_lock)]`, same as before the move) because
//! `execute_approval_with_fee` needs `&mut WalletState` for the whole
//! MCP-approved sign+broadcast. It is the only job that takes the lock *inside*
//! the `async` block rather than before `block_on`, so it is the only one the
//! clippy lint can see; `jobs::send::send_stealth` and three `jobs::refresh`
//! jobs hold it across `block_on` too, without tripping the lint. See
//! `TASKS.md` — none of this is re-scopable without an interface change here.

use std::sync::{Arc, Mutex};

use tokio::runtime::Handle;
use vaughan_core::chains::Fee;
use vaughan_core::core::{proposal::TxProposal, WalletState};
use vaughan_core::error::WalletError;

use crate::provider::{execute_approval_with_fee, ApprovalKind};

use super::UiJobResult;

pub(crate) fn mcp_queued_approve(
    wallet: &Arc<Mutex<WalletState>>,
    handle: &Handle,
    proposal_id: String,
    source: String,
    proposal: TxProposal,
    fee_override: Option<Fee>,
) -> UiJobResult {
    let proposal_for_flash = proposal.clone();
    let kind = ApprovalKind::McpProposal {
        proposal_id: proposal_id.clone(),
        source,
        proposal: Box::new(proposal),
    };
    #[allow(clippy::await_holding_lock)]
    let result = handle.block_on(async {
        let mut w = wallet.lock().unwrap_or_else(|e| e.into_inner());
        execute_approval_with_fee(&kind, &mut w, fee_override.as_ref())
            .await
            .map_err(|e| WalletError::Other(e.to_string()))
    });
    UiJobResult::McpQueuedApprove {
        result,
        proposal: proposal_for_flash,
        proposal_id,
    }
}
