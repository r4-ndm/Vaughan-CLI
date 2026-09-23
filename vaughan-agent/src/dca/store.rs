//! Atomic persistence for DCA plans (`dca-plans.json`, mode 0600).
//!
//! Not secret material, but integrity matters: a tampered `token_out` redirects
//! buys. Same trust boundary as the session token (same OS user).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::types::{DcaPlan, DcaPlanFile, DcaStatus, DCA_PLANS_JSON};
use crate::error::AgentError;

fn plans_path(dir: &Path) -> PathBuf {
    dir.join(DCA_PLANS_JSON)
}

/// Load plans from `dir/dca-plans.json`, or an empty file if missing.
pub fn load_plans(dir: &Path) -> Result<DcaPlanFile, AgentError> {
    let path = plans_path(dir);
    if !path.is_file() {
        return Ok(DcaPlanFile::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| {
        AgentError::ProviderError(format!("failed to read {}: {e}", path.display()))
    })?;
    let file: DcaPlanFile = serde_json::from_str(&raw).map_err(|e| {
        AgentError::ProviderError(format!(
            "invalid {DCA_PLANS_JSON}: {e} (fail-closed — fix or remove the file)"
        ))
    })?;
    Ok(file)
}

/// Atomically replace `dca-plans.json` (tmp + rename, 0600 on Unix).
pub fn save_plans(dir: &Path, file: &DcaPlanFile) -> Result<(), AgentError> {
    fs::create_dir_all(dir).map_err(|e| {
        AgentError::ProviderError(format!("failed to create {}: {e}", dir.display()))
    })?;
    let path = plans_path(dir);
    let tmp = dir.join(format!(".{DCA_PLANS_JSON}.tmp"));
    let body = serde_json::to_string_pretty(file)
        .map_err(|e| AgentError::ProviderError(format!("serialize dca plans: {e}")))?;
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(|e| {
            AgentError::ProviderError(format!("failed to write {}: {e}", tmp.display()))
        })?;
        f.write_all(body.as_bytes()).map_err(|e| {
            AgentError::ProviderError(format!("failed to write {}: {e}", tmp.display()))
        })?;
    }
    fs::rename(&tmp, &path).map_err(|e| {
        AgentError::ProviderError(format!("failed to replace {}: {e}", path.display()))
    })?;
    Ok(())
}

/// Append a new active plan and persist.
pub fn add_plan(dir: &Path, plan: DcaPlan) -> Result<DcaPlan, AgentError> {
    let mut file = load_plans(dir)?;
    if file.plans.iter().any(|p| p.id == plan.id) {
        return Err(AgentError::InvalidToolCall(format!(
            "dca plan id `{}` already exists",
            plan.id
        )));
    }
    file.plans.push(plan.clone());
    save_plans(dir, &file)?;
    Ok(plan)
}

/// Mutate one plan by id; persist on success.
pub fn update_plan<F>(dir: &Path, id: &str, f: F) -> Result<DcaPlan, AgentError>
where
    F: FnOnce(&mut DcaPlan) -> Result<(), AgentError>,
{
    let mut file = load_plans(dir)?;
    let plan = file
        .plans
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| AgentError::InvalidToolCall(format!("dca plan `{id}` not found")))?;
    f(plan)?;
    let out = plan.clone();
    save_plans(dir, &file)?;
    Ok(out)
}

/// Cancel an active/paused plan (stops future slices).
pub fn cancel_plan(dir: &Path, id: &str) -> Result<DcaPlan, AgentError> {
    update_plan(dir, id, |p| {
        if matches!(p.status, DcaStatus::Cancelled | DcaStatus::Done) {
            return Err(AgentError::InvalidToolCall(format!(
                "plan `{id}` is already {}",
                p.status.as_str()
            )));
        }
        p.status = DcaStatus::Cancelled;
        Ok(())
    })
}

/// Pause or resume.
pub fn set_paused(dir: &Path, id: &str, paused: bool) -> Result<DcaPlan, AgentError> {
    update_plan(dir, id, |p| {
        match (paused, p.status) {
            (true, DcaStatus::Active) => p.status = DcaStatus::Paused,
            (false, DcaStatus::Paused) => p.status = DcaStatus::Active,
            (true, _) => {
                return Err(AgentError::InvalidToolCall(format!(
                    "cannot pause plan in status {}",
                    p.status.as_str()
                )));
            }
            (false, _) => {
                return Err(AgentError::InvalidToolCall(format!(
                    "cannot resume plan in status {}",
                    p.status.as_str()
                )));
            }
        }
        Ok(())
    })
}

/// Replace the full plan list after an in-memory engine tick (single write).
pub fn replace_all(dir: &Path, file: &DcaPlanFile) -> Result<(), AgentError> {
    save_plans(dir, file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dca::types::{DcaTrigger, DcaVenue};
    use tempfile::tempdir;

    fn sample_plan(id: &str) -> DcaPlan {
        DcaPlan {
            id: id.into(),
            chain_id: 369,
            account: "0x1111111111111111111111111111111111111111".into(),
            token_out: "0x2222222222222222222222222222222222222222".into(),
            slice_amount_wei: "1000000000000000000".into(),
            trigger: DcaTrigger::Time {
                interval_secs: 14400,
            },
            venue: DcaVenue::default(),
            max_slippage_bps: 100,
            max_slices: 10,
            slices_done: 0,
            spent_wei: "0".into(),
            consecutive_failures: 0,
            status: DcaStatus::Active,
            created_at: 1_700_000_000,
            next_due_at: 1_700_000_000,
            dry_run: true,
            slice_log: vec![],
        }
    }

    #[test]
    fn roundtrip_and_unknown_field_fails() {
        let dir = tempdir().unwrap();
        add_plan(dir.path(), sample_plan("dca-1")).unwrap();
        let loaded = load_plans(dir.path()).unwrap();
        assert_eq!(loaded.plans.len(), 1);
        assert_eq!(loaded.plans[0].id, "dca-1");

        std::fs::write(
            dir.path().join(DCA_PLANS_JSON),
            r#"{"plans":[{"id":"x","token_out":"0x22","slice_amount_wei":"1","trigger":{"kind":"time","interval_secs":900},"venue":"auto","max_slippage_bps":100,"max_slices":1,"created_at":1,"next_due_at":1,"typo_field":true}]}"#,
        )
        .unwrap();
        assert!(load_plans(dir.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn saved_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        add_plan(dir.path(), sample_plan("dca-2")).unwrap();
        let mode = std::fs::metadata(dir.path().join(DCA_PLANS_JSON))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn cancel_and_pause() {
        let dir = tempdir().unwrap();
        add_plan(dir.path(), sample_plan("dca-3")).unwrap();
        set_paused(dir.path(), "dca-3", true).unwrap();
        assert_eq!(
            load_plans(dir.path()).unwrap().plans[0].status,
            DcaStatus::Paused
        );
        set_paused(dir.path(), "dca-3", false).unwrap();
        cancel_plan(dir.path(), "dca-3").unwrap();
        assert_eq!(
            load_plans(dir.path()).unwrap().plans[0].status,
            DcaStatus::Cancelled
        );
    }
}
