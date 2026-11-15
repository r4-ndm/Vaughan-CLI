//! Integration tests for read-only `policy_pending` and shared step-up flow (S3c slice C1).
//!
//! Covers fail-closed read-only inspection (`pending: null` / `approval_present: false`),
//! symlink/mode/UID/hardlink trust checks without file mutation or consumption,
//! peer-UID gating before dispatch, policy-body omission, and end-to-end
//! `stage -> poll -> apply` via `vaughan_core::core::policy_step_up` against a real `Signer`.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use alloy::primitives::Address;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use vaughan_agent::sentient::{load_policy, save_policy, AgentSessionPolicy};
use vaughan_cli::signerd::approval::{
    PendingPolicy, POLICY_APPROVAL_FILENAME, POLICY_PENDING_FILENAME,
};
use vaughan_cli::signerd::{
    is_peer_uid_allowed, stage_pending_policy_with_replace, write_approval, Approval,
    MockBroadcaster, MockSwapQuoter, Signer, SignerClient, SignerResponse,
};
use vaughan_core::core::policy_step_up::{apply, poll, stage, StepUpState};

struct TestFixture {
    _dir: TempDir,
    store: PathBuf,
    signer: Signer,
    uid: u32,
    base_policy: AgentSessionPolicy,
}

fn setup_fixture(daily_cap_pct: u8) -> TestFixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

    let base_policy = AgentSessionPolicy {
        daily_cap_pct,
        max_position_pct: 50,
        required_rpc_quorum: 1,
        ..AgentSessionPolicy::default()
    };
    save_policy(dir.path(), &base_policy).unwrap();

    let uid = rustix::process::geteuid().as_raw();
    let signer = Signer::new(
        dir.path().to_path_buf(),
        Address::from([0x11; 20]),
        943,
        "http://127.0.0.1:8545".to_string(),
        Arc::new(MockSwapQuoter::new()),
        Arc::new(MockBroadcaster::new()),
    );
    signer.set_expected_approval_uid(uid);

    let store = dir.path().to_path_buf();
    TestFixture {
        _dir: dir,
        store,
        signer,
        uid,
        base_policy,
    }
}

fn draft_policy(base: &AgentSessionPolicy, daily_cap_pct: u8) -> AgentSessionPolicy {
    let mut next = base.clone();
    next.daily_cap_pct = daily_cap_pct;
    next
}

fn stage_on_disk(store: &Path, policy: &AgentSessionPolicy) -> PathBuf {
    let now = vaughan_agent::dca::now_unix();
    stage_pending_policy_with_replace(store, policy, now, true).unwrap();
    store.join(POLICY_PENDING_FILENAME)
}

fn write_trusted_approval(
    store: &Path,
    old: &AgentSessionPolicy,
    new: &AgentSessionPolicy,
    ttl_secs: u64,
) -> PathBuf {
    let now = vaughan_agent::dca::now_unix();
    write_approval(
        store,
        &Approval {
            old_digest: old.digest(),
            new_digest: new.digest(),
            issued_at_unix: now,
            expires_at_unix: now.saturating_add(ttl_secs),
        },
    )
    .unwrap()
}

struct SocketHarness {
    _sock_dir: TempDir,
    sock_path: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for SocketHarness {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_socket_server(signer: Signer, allow_uid: Vec<u32>) -> SocketHarness {
    let sock_dir = tempfile::tempdir().unwrap();
    let sock_path = sock_dir.path().join("signerd-pending.sock");
    let listener = UnixListener::bind(&sock_path).unwrap();

    let task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let signer = signer.clone();
            let allow = allow_uid.clone();
            tokio::spawn(async move {
                let Ok(cred) = stream.peer_cred() else {
                    return;
                };
                let peer_uid = cred.uid();
                if !is_peer_uid_allowed(peer_uid, &allow) {
                    let msg = serde_json::to_string(&SignerResponse::error(
                        "unauthorized",
                        "peer UID not permitted",
                    ))
                    .unwrap()
                        + "\n";
                    let _ = stream.write_all(msg.as_bytes()).await;
                    return;
                }
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                    let resp = signer.handle_line_with_uid(line.trim(), peer_uid).await;
                    let _ = stream.write_all(format!("{resp}\n").as_bytes()).await;
                }
            });
        }
    });

    SocketHarness {
        _sock_dir: sock_dir,
        sock_path,
        task,
    }
}

async fn query_pending(signer: &Signer, uid: u32) -> SignerResponse {
    let raw = signer
        .handle_line_with_uid(r#"{"op":"policy_pending"}"#, uid)
        .await;
    serde_json::from_str(&raw).unwrap()
}

#[tokio::test]
async fn test_policy_pending_unauthorized_peer_uid_refused_before_inspecting_files() {
    let fx = setup_fixture(10);
    let next = draft_policy(&fx.base_policy, 40);
    stage_on_disk(&fx.store, &next);
    let approval_path = write_trusted_approval(&fx.store, &fx.base_policy, &next, 120);

    let other_uid = fx.uid.wrapping_add(42_424);
    let srv = spawn_socket_server(fx.signer, vec![other_uid]).await;
    let client = SignerClient::new(&srv.sock_path);

    let resp = client.policy_pending().await.unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.code.as_deref(), Some("unauthorized"));
    assert!(resp.pending.is_none());
    assert!(approval_path.exists());
}

#[tokio::test]
async fn test_policy_pending_returns_none_when_absent_or_expired() {
    let fx = setup_fixture(10);
    let resp_empty = query_pending(&fx.signer, fx.uid).await;
    assert!(resp_empty.ok);
    assert!(resp_empty.pending.is_none());

    let next = draft_policy(&fx.base_policy, 40);
    let now = vaughan_agent::dca::now_unix();
    let expired = PendingPolicy {
        policy: next,
        staged_at_unix: now.saturating_sub(700),
        expires_at_unix: now.saturating_sub(1),
    };
    let pending_path = fx.store.join(POLICY_PENDING_FILENAME);
    std::fs::write(&pending_path, serde_json::to_string(&expired).unwrap()).unwrap();
    std::fs::set_permissions(&pending_path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let resp_expired = query_pending(&fx.signer, fx.uid).await;
    assert!(resp_expired.ok);
    assert!(resp_expired.pending.is_none());
}

#[tokio::test]
async fn test_policy_pending_returns_none_when_pending_file_is_symlinked_or_corrupt() {
    let fx = setup_fixture(10);
    let next = draft_policy(&fx.base_policy, 40);
    let now = vaughan_agent::dca::now_unix();
    let payload = PendingPolicy {
        policy: next,
        staged_at_unix: now,
        expires_at_unix: now + 600,
    };
    let target = fx.store.join("pending-target.json");
    std::fs::write(&target, serde_json::to_string(&payload).unwrap()).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();

    let pending_path = fx.store.join(POLICY_PENDING_FILENAME);
    std::os::unix::fs::symlink(&target, &pending_path).unwrap();

    let resp = query_pending(&fx.signer, fx.uid).await;
    assert!(resp.ok);
    assert!(resp.pending.is_none());
    assert!(std::fs::symlink_metadata(&pending_path).is_ok());

    std::fs::remove_file(&pending_path).unwrap();
    std::fs::write(&pending_path, b"{not json").unwrap();
    std::fs::set_permissions(&pending_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let resp_corrupt = query_pending(&fx.signer, fx.uid).await;
    assert!(resp_corrupt.ok);
    assert!(resp_corrupt.pending.is_none());
}

#[tokio::test]
async fn test_policy_pending_staged_without_approval_reports_false() {
    let fx = setup_fixture(10);
    let next = draft_policy(&fx.base_policy, 40);
    stage_on_disk(&fx.store, &next);

    let resp = query_pending(&fx.signer, fx.uid).await;
    assert!(resp.ok);
    let info = resp.pending.expect("expected pending info");
    assert_eq!(info.policy_digest, next.digest());
    assert!(!info.approval_present);
    assert!(info.expires_at_unix > vaughan_agent::dca::now_unix());
}

#[tokio::test]
async fn test_policy_pending_symlink_or_wrong_mode_approval_reports_false_read_only() {
    let fx = setup_fixture(10);
    let next = draft_policy(&fx.base_policy, 40);
    stage_on_disk(&fx.store, &next);
    let approval_path = fx.store.join(POLICY_APPROVAL_FILENAME);

    let target = fx.store.join("approval-symlink-target.json");
    let now = vaughan_agent::dca::now_unix();
    let valid_json = serde_json::to_string(&Approval {
        old_digest: fx.base_policy.digest(),
        new_digest: next.digest(),
        issued_at_unix: now,
        expires_at_unix: now + 120,
    })
    .unwrap();
    std::fs::write(&target, &valid_json).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o440)).unwrap();
    std::os::unix::fs::symlink(&target, &approval_path).unwrap();
    let r_sym = query_pending(&fx.signer, fx.uid).await;
    assert!(!r_sym.pending.unwrap().approval_present);
    assert!(std::fs::symlink_metadata(&approval_path).is_ok());
    std::fs::remove_file(&approval_path).unwrap();

    write_trusted_approval(&fx.store, &fx.base_policy, &next, 120);
    std::fs::set_permissions(&approval_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let r_mode = query_pending(&fx.signer, fx.uid).await;
    assert!(!r_mode.pending.unwrap().approval_present);
    assert!(approval_path.exists());
}

#[tokio::test]
async fn test_policy_pending_wrong_uid_hardlink_corrupt_or_expired_approval_false() {
    let fx = setup_fixture(10);
    let next = draft_policy(&fx.base_policy, 40);
    stage_on_disk(&fx.store, &next);
    let approval_path = write_trusted_approval(&fx.store, &fx.base_policy, &next, 120);

    // 1. Wrong owner UID
    fx.signer.set_expected_approval_uid(fx.uid.wrapping_add(1));
    assert!(
        !query_pending(&fx.signer, fx.uid)
            .await
            .pending
            .unwrap()
            .approval_present
    );
    fx.signer.set_expected_approval_uid(fx.uid);

    // 2. Hardlink (nlink == 2)
    let extra = fx.store.join("approval-hardlink.json");
    std::fs::hard_link(&approval_path, &extra).unwrap();
    assert!(
        !query_pending(&fx.signer, fx.uid)
            .await
            .pending
            .unwrap()
            .approval_present
    );
    std::fs::remove_file(&extra).unwrap();

    // 3. Corrupt JSON
    std::fs::remove_file(&approval_path).unwrap();
    std::fs::write(&approval_path, b"{corrupt").unwrap();
    std::fs::set_permissions(&approval_path, std::fs::Permissions::from_mode(0o440)).unwrap();
    assert!(
        !query_pending(&fx.signer, fx.uid)
            .await
            .pending
            .unwrap()
            .approval_present
    );

    // 4. Expired approval
    let now = vaughan_agent::dca::now_unix();
    std::fs::remove_file(&approval_path).unwrap();
    write_approval(
        &fx.store,
        &Approval {
            old_digest: fx.base_policy.digest(),
            new_digest: next.digest(),
            issued_at_unix: now.saturating_sub(200),
            expires_at_unix: now.saturating_sub(1),
        },
    )
    .unwrap();
    assert!(
        !query_pending(&fx.signer, fx.uid)
            .await
            .pending
            .unwrap()
            .approval_present
    );
    assert!(approval_path.exists());
}

#[tokio::test]
async fn test_policy_pending_valid_approval_true_preserves_file_and_omits_policy_body() {
    let fx = setup_fixture(10);
    let next = draft_policy(&fx.base_policy, 45);
    stage_on_disk(&fx.store, &next);
    let approval_path = write_trusted_approval(&fx.store, &fx.base_policy, &next, 120);

    let before_bytes = std::fs::read(&approval_path).unwrap();
    let before_mtime = std::fs::metadata(&approval_path)
        .unwrap()
        .modified()
        .unwrap();

    let raw_line = fx
        .signer
        .handle_line_with_uid(r#"{"op":"policy_pending"}"#, fx.uid)
        .await;
    assert!(!raw_line.contains("daily_cap_pct"));
    assert!(!raw_line.contains("allow_transfer_to"));
    assert!(!raw_line.contains(r#""policy":"#));

    let resp: SignerResponse = serde_json::from_str(&raw_line).unwrap();
    assert!(resp.ok);
    assert!(resp.policy.is_none());
    let info = resp.pending.expect("expected pending info");
    assert_eq!(info.policy_digest, next.digest());
    assert!(info.approval_present);

    assert_eq!(std::fs::read(&approval_path).unwrap(), before_bytes);
    assert_eq!(
        std::fs::metadata(&approval_path)
            .unwrap()
            .modified()
            .unwrap(),
        before_mtime
    );
}

#[tokio::test]
async fn test_policy_step_up_e2e_stage_poll_apply_against_real_signer() {
    let fx = setup_fixture(10);
    let next = draft_policy(&fx.base_policy, 65);
    let next_val = serde_json::to_value(&next).unwrap();

    let srv = spawn_socket_server(fx.signer, vec![fx.uid]).await;
    let client = SignerClient::new(&srv.sock_path);

    let staged = stage(&client, &next_val, Some(fx.base_policy.digest()))
        .await
        .unwrap();
    assert_eq!(staged.policy_digest, next.digest());
    assert!(matches!(
        poll(&client, &staged).await.unwrap(),
        StepUpState::Waiting { .. }
    ));

    let approval_path = write_trusted_approval(&fx.store, &fx.base_policy, &next, 120);
    assert_eq!(poll(&client, &staged).await.unwrap(), StepUpState::Ready);
    assert!(approval_path.exists());

    let applied_digest = apply(&client, &staged).await.unwrap();
    assert_eq!(applied_digest, next.digest());
    assert_eq!(load_policy(&fx.store).unwrap(), next);
    assert!(!approval_path.exists());
    assert_eq!(poll(&client, &staged).await.unwrap(), StepUpState::Expired);
}

#[tokio::test]
async fn test_stage_removes_stale_approval_when_prior_pending_absent_or_expired() {
    let fx = setup_fixture(10);
    let p0 = fx.base_policy.clone();
    let p1 = draft_policy(&p0, 35);
    let p2 = draft_policy(&p0, 50);

    // Case A: valid approval for P0 -> P1 on disk with NO policy-pending.json;
    // staging P2 must delete the stale approval file.
    let approval_path = write_trusted_approval(&fx.store, &p0, &p1, 120);
    assert!(approval_path.exists());
    assert!(!fx.store.join(POLICY_PENDING_FILENAME).exists());

    let t0 = vaughan_agent::dca::now_unix();
    vaughan_cli::signerd::approval::stage_pending_policy(&fx.store, &p2, t0).unwrap();
    assert!(
        !approval_path.exists(),
        "staging P2 with no prior pending must delete stale policy-approval.json"
    );

    // Case B: pending P1 staged at t0 plus valid approval for P0 -> P1;
    // after P1's pending TTL expires (via injected `now`), staging P2 with allow_replace=false
    // must delete the stale approval file.
    stage_pending_policy_with_replace(&fx.store, &p1, t0, true).unwrap();
    let approval_path = write_approval(
        &fx.store,
        &Approval {
            old_digest: p0.digest(),
            new_digest: p1.digest(),
            issued_at_unix: t0,
            expires_at_unix: t0
                .saturating_add(vaughan_cli::signerd::approval::PENDING_TTL_SECS)
                .saturating_add(120),
        },
    )
    .unwrap();
    assert!(approval_path.exists());

    let t_after_ttl = t0.saturating_add(vaughan_cli::signerd::approval::PENDING_TTL_SECS + 1);
    vaughan_cli::signerd::approval::stage_pending_policy(&fx.store, &p2, t_after_ttl).unwrap();
    assert!(
        !approval_path.exists(),
        "staging P2 after prior pending expired must delete stale policy-approval.json"
    );
}

#[tokio::test]
async fn test_policy_pending_binds_approval_present_to_staged_and_current_digests() {
    let fx = setup_fixture(10);
    let p0 = fx.base_policy.clone();
    let p1 = draft_policy(&p0, 35);
    let p2 = draft_policy(&p0, 50);
    let p_other_old = draft_policy(&p0, 15);

    stage_on_disk(&fx.store, &p2);

    // 1. Valid unexpired approval minted for P0 -> P1 while P2 is pending -> approval_present=false.
    let approval_path = write_trusted_approval(&fx.store, &p0, &p1, 120);
    let before_bytes = std::fs::read(&approval_path).unwrap();
    let before_mtime = std::fs::metadata(&approval_path)
        .unwrap()
        .modified()
        .unwrap();

    let resp_wrong_new = query_pending(&fx.signer, fx.uid).await;
    assert!(resp_wrong_new.ok);
    assert!(
        !resp_wrong_new.pending.unwrap().approval_present,
        "approval minted for P0 -> P1 must not set approval_present=true when P2 is pending"
    );
    assert_eq!(std::fs::read(&approval_path).unwrap(), before_bytes);
    assert_eq!(
        std::fs::metadata(&approval_path)
            .unwrap()
            .modified()
            .unwrap(),
        before_mtime
    );

    // 2. Valid unexpired approval with mismatched old_digest (p_other_old -> P2) -> approval_present=false.
    std::fs::remove_file(&approval_path).unwrap();
    let approval_path = write_trusted_approval(&fx.store, &p_other_old, &p2, 120);
    let before_bytes = std::fs::read(&approval_path).unwrap();
    let before_mtime = std::fs::metadata(&approval_path)
        .unwrap()
        .modified()
        .unwrap();

    let resp_wrong_old = query_pending(&fx.signer, fx.uid).await;
    assert!(resp_wrong_old.ok);
    assert!(
        !resp_wrong_old.pending.unwrap().approval_present,
        "approval whose old_digest != current on-disk policy digest must set approval_present=false"
    );
    assert_eq!(std::fs::read(&approval_path).unwrap(), before_bytes);
    assert_eq!(
        std::fs::metadata(&approval_path)
            .unwrap()
            .modified()
            .unwrap(),
        before_mtime
    );

    // 3. Valid unexpired approval matching (current P0 -> P2) -> approval_present=true, bytes/mtime unchanged.
    std::fs::remove_file(&approval_path).unwrap();
    let approval_path = write_trusted_approval(&fx.store, &p0, &p2, 120);
    let before_bytes = std::fs::read(&approval_path).unwrap();
    let before_mtime = std::fs::metadata(&approval_path)
        .unwrap()
        .modified()
        .unwrap();

    let resp_match = query_pending(&fx.signer, fx.uid).await;
    assert!(resp_match.ok);
    assert!(
        resp_match.pending.unwrap().approval_present,
        "approval matching (current -> pending) must set approval_present=true"
    );
    assert_eq!(std::fs::read(&approval_path).unwrap(), before_bytes);
    assert_eq!(
        std::fs::metadata(&approval_path)
            .unwrap()
            .modified()
            .unwrap(),
        before_mtime
    );
}
