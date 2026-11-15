//! Integration tests for CLI `vaughan policy` subcommands and idempotent daemon re-staging (S3c slice C2).
//!
//! Spawns an in-process `Signer` on a temporary Unix domain socket with a temporary
//! store (`set_expected_approval_uid(current uid)`) and drives the real compiled
//! `vaughan` binary via `VAUGHAN_SIGNER_SOCKET`, verifying disk state rather than stdout alone.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::Address;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use vaughan_agent::sentient::{load_policy, save_policy, AgentSessionPolicy};
use vaughan_cli::signerd::approval::POLICY_PENDING_FILENAME;
use vaughan_cli::signerd::{
    is_peer_uid_allowed, write_approval, Approval, MockBroadcaster, MockSwapQuoter, Signer,
    SignerClient, SignerResponse,
};

struct PolicyCliFixture {
    store_dir: TempDir,
    sock_dir: TempDir,
    sock_path: PathBuf,
    signer: Signer,
    uid: u32,
    base_policy: AgentSessionPolicy,
    server_task: tokio::task::JoinHandle<()>,
}

impl Drop for PolicyCliFixture {
    fn drop(&mut self) {
        self.server_task.abort();
    }
}

fn spawn_socket_loop(
    listener: UnixListener,
    signer: Signer,
    uid: u32,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let s = signer.clone();
            tokio::spawn(async move {
                let Ok(cred) = stream.peer_cred() else {
                    return;
                };
                if !is_peer_uid_allowed(cred.uid(), &[uid]) {
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
                    let resp = s.handle_line_with_uid(line.trim(), cred.uid()).await;
                    let _ = stream.write_all(format!("{resp}\n").as_bytes()).await;
                }
            });
        }
    })
}

async fn setup_cli_fixture(daily_cap_pct: u8) -> PolicyCliFixture {
    let store_dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(store_dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

    let base_policy = AgentSessionPolicy {
        daily_cap_pct,
        max_position_pct: 50,
        required_rpc_quorum: 1,
        ..AgentSessionPolicy::default()
    };
    save_policy(store_dir.path(), &base_policy).unwrap();

    let uid = rustix::process::geteuid().as_raw();
    let signer = Signer::new(
        store_dir.path().to_path_buf(),
        Address::from([0x11; 20]),
        943,
        "http://127.0.0.1:8545".to_string(),
        Arc::new(MockSwapQuoter::new()),
        Arc::new(MockBroadcaster::new()),
    );
    signer.set_expected_approval_uid(uid);

    let sock_dir = tempfile::tempdir().unwrap();
    let sock_path = sock_dir.path().join("signerd-cli.sock");
    let listener = UnixListener::bind(&sock_path).unwrap();
    let server_task = spawn_socket_loop(listener, signer.clone(), uid);

    PolicyCliFixture {
        store_dir,
        sock_dir,
        sock_path,
        signer,
        uid,
        base_policy,
        server_task,
    }
}

fn build_vaughan_cmd(home: &Path, sock_path: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_vaughan"));
    cmd.args(args)
        .env("HOME", home)
        .env("VAUGHAN_SIGNER_SOCKET", sock_path)
        .env_remove("VAUGHAN_PASSWORD")
        .stdin(Stdio::null());
    cmd
}

async fn run_vaughan(home: &Path, sock_path: &Path, args: &[&str]) -> std::process::Output {
    let home = home.to_path_buf();
    let sock = sock_path.to_path_buf();
    let owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        build_vaughan_cmd(&home, &sock, &refs).output().unwrap()
    })
    .await
    .unwrap()
}

fn approve_when_staged(
    sock_path: PathBuf,
    store: PathBuf,
    old: AgentSessionPolicy,
    new: AgentSessionPolicy,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let client = SignerClient::new(&sock_path);
        let target_digest = new.digest();
        for _ in 0..100 {
            if let Ok(resp) = client.policy_pending().await {
                if let Some(info) = resp.pending {
                    if info.policy_digest == target_digest && !info.approval_present {
                        let now = vaughan_agent::dca::now_unix();
                        write_approval(
                            &store,
                            &Approval {
                                old_digest: old.digest(),
                                new_digest: target_digest,
                                issued_at_unix: now,
                                expires_at_unix: now.saturating_add(120),
                            },
                        )
                        .unwrap();
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for staged pending digest {target_digest}");
    })
}

fn draft_cap(base: &AgentSessionPolicy, daily_cap_pct: u8) -> AgentSessionPolicy {
    let mut next = base.clone();
    next.daily_cap_pct = daily_cap_pct;
    next
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_daemon_restage_identical_digest_is_idempotent_and_different_non_tightening_refused() {
    let fx = setup_cli_fixture(30).await;
    let draft_same = draft_cap(&fx.base_policy, 60);
    let draft_other = draft_cap(&fx.base_policy, 75);
    let client = SignerClient::new(&fx.sock_path);

    let first = client
        .policy_stage(
            serde_json::to_value(&draft_same).unwrap(),
            Some(fx.base_policy.digest()),
        )
        .await
        .unwrap();
    assert_eq!(first.code.as_deref(), Some("step_up_required"));
    let first_expiry = first.pending.expect("pending info").expires_at_unix;
    let pending_path = fx.store_dir.path().join(POLICY_PENDING_FILENAME);
    let first_bytes = std::fs::read(&pending_path).unwrap();

    // Re-staging the identical non-tightening policy is idempotent and keeps the same TTL/bytes.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let second = client
        .policy_stage(
            serde_json::to_value(&draft_same).unwrap(),
            Some(fx.base_policy.digest()),
        )
        .await
        .unwrap();
    assert_eq!(second.code.as_deref(), Some("step_up_required"));
    assert_eq!(
        second.pending.expect("pending info").expires_at_unix,
        first_expiry
    );
    assert_eq!(std::fs::read(&pending_path).unwrap(), first_bytes);

    // Staging a different non-tightening policy while pending is still refused (`step_up_pending`).
    let third = client
        .policy_stage(
            serde_json::to_value(&draft_other).unwrap(),
            Some(fx.base_policy.digest()),
        )
        .await
        .unwrap();
    assert_eq!(third.code.as_deref(), Some("step_up_pending"));
    assert_eq!(std::fs::read(&pending_path).unwrap(), first_bytes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cli_policy_set_stages_waits_for_approval_and_applies_to_disk() {
    let fx = setup_cli_fixture(50).await;
    let next = draft_cap(&fx.base_policy, 10);
    let approver = approve_when_staged(
        fx.sock_path.clone(),
        fx.store_dir.path().to_path_buf(),
        fx.base_policy.clone(),
        next.clone(),
    );

    let out = run_vaughan(
        fx.sock_dir.path(),
        &fx.sock_path,
        &[
            "--profile",
            "sentient",
            "policy",
            "set",
            "--daily-cap-pct",
            "10",
            "--wait-secs",
            "10",
        ],
    )
    .await;
    approver.await.unwrap();

    assert!(
        out.status.success(),
        "expected exit 0, got {:?}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&next.digest()), "stdout: {stdout}");
    let disk = load_policy(fx.store_dir.path()).unwrap();
    assert_eq!(disk.daily_cap_pct, 10);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cli_policy_set_timeout_leaves_disk_unchanged_and_identical_rerun_succeeds() {
    let fx = setup_cli_fixture(30).await;
    let policy_path = fx
        .store_dir
        .path()
        .join(vaughan_agent::sentient::SENTIENT_POLICY_TOML);
    let baseline_bytes = std::fs::read(&policy_path).unwrap();

    let timed_args = [
        "--profile",
        "sentient",
        "policy",
        "set",
        "--daily-cap-pct",
        "60",
        "--wait-secs",
        "2",
    ];
    let timed_out = run_vaughan(fx.sock_dir.path(), &fx.sock_path, &timed_args).await;
    assert!(!timed_out.status.success());
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline_bytes);
    let stderr = String::from_utf8_lossy(&timed_out.stderr);
    assert!(
        stderr.contains("sudo vaughan signerd approve"),
        "stderr: {stderr}"
    );

    let next = draft_cap(&fx.base_policy, 60);
    let approver = approve_when_staged(
        fx.sock_path.clone(),
        fx.store_dir.path().to_path_buf(),
        fx.base_policy.clone(),
        next,
    );
    let rerun_args = [
        "--profile",
        "sentient",
        "policy",
        "set",
        "--daily-cap-pct",
        "60",
        "--wait-secs",
        "10",
    ];
    let rerun = run_vaughan(fx.sock_dir.path(), &fx.sock_path, &rerun_args).await;
    approver.await.unwrap();
    assert!(
        rerun.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&rerun.stderr)
    );
    assert_eq!(load_policy(fx.store_dir.path()).unwrap().daily_cap_pct, 60);
}

async fn assert_pending_human_and_json_none(sock_dir: &Path, sock_path: &Path) {
    let none_out = run_vaughan(sock_dir, sock_path, &["policy", "pending"]).await;
    assert!(none_out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&none_out.stdout).trim(),
        "no staged update"
    );
    let none_json = run_vaughan(sock_dir, sock_path, &["--json", "policy", "pending"]).await;
    let parsed_none: serde_json::Value = serde_json::from_slice(&none_json.stdout).unwrap();
    assert_eq!(parsed_none["data"]["pending"], serde_json::Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cli_policy_pending_outputs_none_waiting_and_approved_human_and_json() {
    let fx = setup_cli_fixture(30).await;
    let next = draft_cap(&fx.base_policy, 55);
    assert_pending_human_and_json_none(fx.sock_dir.path(), &fx.sock_path).await;

    let stage_resp = SignerClient::new(&fx.sock_path)
        .policy_stage(serde_json::to_value(&next).unwrap(), None)
        .await
        .unwrap();
    assert_eq!(stage_resp.code.as_deref(), Some("step_up_required"));

    let wait_out = run_vaughan(fx.sock_dir.path(), &fx.sock_path, &["policy", "pending"]).await;
    let wait_str = String::from_utf8_lossy(&wait_out.stdout);
    assert!(wait_str.contains(&next.digest()), "stdout: {wait_str}");
    assert!(wait_str.contains("s left)"), "stdout: {wait_str}");
    assert!(
        wait_str.contains("approval_present: false"),
        "stdout: {wait_str}"
    );

    let now = vaughan_agent::dca::now_unix();
    write_approval(
        fx.store_dir.path(),
        &Approval {
            old_digest: fx.base_policy.digest(),
            new_digest: next.digest(),
            issued_at_unix: now,
            expires_at_unix: now.saturating_add(120),
        },
    )
    .unwrap();
    let app_json = run_vaughan(
        fx.sock_dir.path(),
        &fx.sock_path,
        &["--json", "policy", "pending"],
    )
    .await;
    let parsed_app: serde_json::Value = serde_json::from_slice(&app_json.stdout).unwrap();
    assert_eq!(parsed_app["data"]["policy_digest"], next.digest());
    assert_eq!(parsed_app["data"]["approval_present"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cli_policy_show_human_and_json_unchanged_byte_for_byte() {
    let fx = setup_cli_fixture(25).await;
    let digest = fx.base_policy.digest();
    let expected_human = format!(
        "policy_digest: {digest}\n{}",
        toml::to_string_pretty(&fx.base_policy).unwrap()
    );
    let expected_json = format!(
        "{}\n",
        serde_json::to_string_pretty(&serde_json::json!({
            "ok": true,
            "data": {
                "policy": serde_json::to_value(&fx.base_policy).unwrap(),
                "policy_digest": digest,
            }
        }))
        .unwrap()
    );

    let human = run_vaughan(fx.sock_dir.path(), &fx.sock_path, &["policy", "show"]).await;
    assert!(human.status.success());
    assert_eq!(String::from_utf8_lossy(&human.stdout), expected_human);

    let json = run_vaughan(
        fx.sock_dir.path(),
        &fx.sock_path,
        &["--json", "policy", "show"],
    )
    .await;
    assert!(json.status.success());
    assert_eq!(String::from_utf8_lossy(&json.stdout), expected_json);
}

fn interrupt_child_on_staged(
    pid: rustix::process::Pid,
    stderr_pipe: std::process::ChildStderr,
) -> tokio::task::JoinHandle<String> {
    use std::io::{BufRead, Read};

    tokio::task::spawn_blocking(move || {
        let mut r = std::io::BufReader::new(stderr_pipe);
        let mut captured = String::new();
        let mut line = String::new();
        while r.read_line(&mut line).unwrap_or(0) > 0 {
            captured.push_str(&line);
            if line.contains("Staged.") {
                rustix::process::kill_process(pid, rustix::process::Signal::Int).unwrap();
                break;
            }
            line.clear();
        }
        let _ = r.read_to_string(&mut captured);
        captured
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cli_policy_set_ctrl_c_interrupt_leaves_disk_unchanged() {
    let fx = setup_cli_fixture(40).await;
    let _ = (fx.uid, &fx.signer);
    let policy_path = fx
        .store_dir
        .path()
        .join(vaughan_agent::sentient::SENTIENT_POLICY_TOML);
    let baseline = std::fs::read(&policy_path).unwrap();

    let args = [
        "--profile",
        "sentient",
        "policy",
        "set",
        "--daily-cap-pct",
        "70",
        "--wait-secs",
        "60",
    ];
    let mut child = build_vaughan_cmd(fx.sock_dir.path(), &fx.sock_path, &args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
    let reader_task = interrupt_child_on_staged(pid, child.stderr.take().unwrap());

    let status = tokio::task::spawn_blocking(move || child.wait().unwrap())
        .await
        .unwrap();
    let stderr = reader_task.await.unwrap();
    assert!(!status.success());
    assert!(
        stderr.contains("not applied; the staged update stays pending until it expires (10 min) or you re-run the same command"),
        "stderr: {stderr}"
    );
    assert_eq!(std::fs::read(&policy_path).unwrap(), baseline);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cli_policy_confirm_unsafe_updates_acknowledge_unsafe_on_disk() {
    let fx = setup_cli_fixture(40).await;
    let mut unsafe_next = fx.base_policy.clone();
    unsafe_next.acknowledge_unsafe = true;
    let approver = approve_when_staged(
        fx.sock_path.clone(),
        fx.store_dir.path().to_path_buf(),
        fx.base_policy.clone(),
        unsafe_next,
    );
    let confirm_out = run_vaughan(
        fx.sock_dir.path(),
        &fx.sock_path,
        &[
            "--profile",
            "sentient",
            "policy",
            "confirm-unsafe",
            "--wait-secs",
            "10",
        ],
    )
    .await;
    approver.await.unwrap();
    assert!(
        confirm_out.status.success(),
        "confirm-unsafe stderr: {}",
        String::from_utf8_lossy(&confirm_out.stderr)
    );
    assert!(load_policy(fx.store_dir.path()).unwrap().acknowledge_unsafe);
}
