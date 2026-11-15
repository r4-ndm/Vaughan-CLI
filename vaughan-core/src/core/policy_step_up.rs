//! Shared client-side policy step-up orchestration (`stage -> poll -> apply`).
//!
//! Purpose: single implementation of the out-of-band human approval policy
//! update flow against `vaughan signerd`.
//! Called by: CLI policy commands (`vaughan --profile sentient policy set` / `confirm-unsafe`)
//! and TUI policy screens.
//! Must NOT live here: vault password handling, filesystem writes to signer
//! store files (`policy-pending.json` / `policy-approval.json`), UI rendering,
//! or CLI argument parsing.

use std::fmt;
use std::time::{Duration, Instant};

use crate::core::signer_client::{PendingInfo, SignerClient, SignerResponse};

/// Error returned by the shared client-side policy step-up flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepUpError {
    /// Signer daemon rejected the request with a structured error code and message.
    Daemon {
        /// Machine-readable error code returned by `vaughan signerd`.
        code: String,
        /// Human-readable error explanation returned by `vaughan signerd`.
        message: String,
    },
    /// Socket connection, framing, timeout, or response decoding failure.
    Transport(String),
    /// No matching staged policy exists on the daemon for the client's digest.
    NotStaged,
    /// The staged policy update expired before approval and application completed.
    Expired,
    /// Polling exceeded the caller's timeout while waiting for root approval.
    TimedOut,
    /// Daemon refused `auth: "approved"` (`step_up_failed`: missing, invalid, or unapproved).
    NotApproved,
}

impl fmt::Display for StepUpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Daemon { code, message } => match code.as_str() {
                "step_up_pending" => write!(
                    f,
                    "{message}; finish or let the staged update expire, then run: sudo vaughan signerd approve"
                ),
                "policy_conflict" => write!(
                    f,
                    "{message}; reload the active policy digest and retry"
                ),
                _ => write!(f, "signer daemon refused step-up ({code}): {message}"),
            },
            Self::Transport(msg) => write!(
                f,
                "signer transport error: {msg}; verify vaughan-signerd is running"
            ),
            Self::NotStaged => write!(
                f,
                "no matching policy update is staged on the signer daemon; stage the policy first"
            ),
            Self::Expired => write!(
                f,
                "staged policy update expired; re-stage the policy and run: sudo vaughan signerd approve"
            ),
            Self::TimedOut => write!(
                f,
                "timed out waiting for root approval; run `sudo vaughan signerd approve` or `vaughan policy pending`"
            ),
            Self::NotApproved => write!(
                f,
                "policy update was not approved or approval was rejected; run `sudo vaughan signerd approve` and retry"
            ),
        }
    }
}

impl std::error::Error for StepUpError {}

/// Staged policy handle held by the client between [`stage`], [`poll`], and [`apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    /// Client's original draft policy payload (re-sent on [`apply`] rather than trusting daemon state).
    pub policy: serde_json::Value,
    /// Optional digest of the policy the client expects to replace.
    pub expected_digest: Option<String>,
    /// Canonical SHA-256 hex digest of the staged policy.
    pub policy_digest: String,
    /// Unix timestamp when the staged slot expires (`0` if omitted by daemon).
    pub expires_at_unix: u64,
}

/// Current state observed by [`poll`] for a [`Staged`] policy update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepUpState {
    /// Staged policy exists and is awaiting `sudo vaughan signerd approve`.
    Waiting {
        /// Unix timestamp when the staged pending slot expires.
        expires_at_unix: u64,
    },
    /// Valid, trusted approval file is present and the client may call [`apply`].
    Ready,
    /// Staged pending slot is absent or expired on the daemon.
    Expired,
}

fn daemon_error(resp: &SignerResponse) -> StepUpError {
    let code = resp
        .code
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("signer_refused")
        .to_string();
    let message = resp
        .message
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&code)
        .to_string();
    StepUpError::Daemon { code, message }
}

fn extract_staged_digest(resp: &SignerResponse) -> Result<String, StepUpError> {
    resp.policy_digest
        .clone()
        .or_else(|| resp.pending.as_ref().map(|p| p.policy_digest.clone()))
        .filter(|d| !d.is_empty())
        .ok_or_else(|| {
            StepUpError::Transport(
                "invalid_signer_response: stage response missing policy_digest".to_string(),
            )
        })
}

/// Stage a draft policy on the signer daemon (`auth: "stage"`).
pub async fn stage(
    client: &SignerClient,
    policy: &serde_json::Value,
    expected_digest: Option<String>,
) -> Result<Staged, StepUpError> {
    let resp = client
        .policy_stage(policy.clone(), expected_digest.clone())
        .await
        .map_err(StepUpError::Transport)?;

    if resp.code.as_deref() == Some("step_up_required") {
        let policy_digest = extract_staged_digest(&resp)?;
        let expires_at_unix = resp.pending.as_ref().map_or(0, |p| p.expires_at_unix);
        return Ok(Staged {
            policy: policy.clone(),
            expected_digest,
            policy_digest,
            expires_at_unix,
        });
    }

    Err(daemon_error(&resp))
}

fn map_pending_info(staged: &Staged, info: PendingInfo) -> Result<StepUpState, StepUpError> {
    if info.policy_digest != staged.policy_digest {
        return Err(StepUpError::NotStaged);
    }
    if info.approval_present {
        return Ok(StepUpState::Ready);
    }
    Ok(StepUpState::Waiting {
        expires_at_unix: info.expires_at_unix,
    })
}

/// Poll the daemon's read-only `policy_pending` endpoint for approval status.
///
/// Uses only `policy_pending` (never `auth: "approved"`), so polling never triggers
/// the step-up rate limiter or consumes an approval.
pub async fn poll(client: &SignerClient, staged: &Staged) -> Result<StepUpState, StepUpError> {
    if staged.policy_digest.is_empty() {
        return Err(StepUpError::NotStaged);
    }

    let resp = client
        .policy_pending()
        .await
        .map_err(StepUpError::Transport)?;

    if !resp.ok {
        return Err(daemon_error(&resp));
    }

    match resp.pending {
        None => Ok(StepUpState::Expired),
        Some(info) => map_pending_info(staged, info),
    }
}

/// Apply the client's staged policy after root approval (`auth: "approved"`).
///
/// Sends the client's own `staged.policy` draft and `expected_digest` so the daemon's
/// pending digest match, approval binding, validation, and TOCTOU checks remain authoritative.
pub async fn apply(client: &SignerClient, staged: &Staged) -> Result<String, StepUpError> {
    if staged.policy_digest.is_empty() {
        return Err(StepUpError::NotStaged);
    }

    let resp = client
        .policy_approved(staged.policy.clone(), staged.expected_digest.clone())
        .await
        .map_err(StepUpError::Transport)?;

    if resp.ok {
        return resp.policy_digest.filter(|d| !d.is_empty()).ok_or_else(|| {
            StepUpError::Transport(
                "invalid_signer_response: policy_approved response missing policy_digest"
                    .to_string(),
            )
        });
    }

    if resp.code.as_deref() == Some("step_up_failed") {
        return Err(StepUpError::NotApproved);
    }

    Err(daemon_error(&resp))
}

async fn run_poll_loop<E, W>(
    client: &SignerClient,
    staged: &Staged,
    interval: Duration,
    timeout: Duration,
    mut elapsed: E,
    mut on_wait: W,
) -> Result<String, StepUpError>
where
    E: FnMut() -> Duration,
    W: FnMut(&Staged, &StepUpState),
{
    loop {
        let state = poll(client, staged).await?;
        match state {
            StepUpState::Ready => return apply(client, staged).await,
            StepUpState::Expired => return Err(StepUpError::Expired),
            StepUpState::Waiting { .. } => {
                on_wait(staged, &state);
                if elapsed() >= timeout {
                    return Err(StepUpError::TimedOut);
                }
                if !interval.is_zero() {
                    tokio::time::sleep(interval).await;
                }
            }
        }
    }
}

/// Stage, poll until approved (or expired/timed out), and apply using an injected unix clock (seconds).
///
/// Passing an explicit clock closure allows deterministic tests without real-time sleeps.
pub async fn run_to_completion_with_clock<C, W>(
    client: &SignerClient,
    policy: serde_json::Value,
    expected_digest: Option<String>,
    interval: Duration,
    timeout: Duration,
    mut now_unix: C,
    on_wait: W,
) -> Result<String, StepUpError>
where
    C: FnMut() -> u64,
    W: FnMut(&Staged, &StepUpState),
{
    let staged = stage(client, &policy, expected_digest).await?;
    let started = now_unix();
    let elapsed = || Duration::from_secs(now_unix().saturating_sub(started));
    run_poll_loop(client, &staged, interval, timeout, elapsed, on_wait).await
}

/// Stage, poll `policy_pending` on `interval` until `Ready` or `timeout`, and apply.
pub async fn run_to_completion<W>(
    client: &SignerClient,
    policy: serde_json::Value,
    expected_digest: Option<String>,
    interval: Duration,
    timeout: Duration,
    on_wait: W,
) -> Result<String, StepUpError>
where
    W: FnMut(&Staged, &StepUpState),
{
    let staged = stage(client, &policy, expected_digest).await?;
    let started = Instant::now();
    let elapsed = || started.elapsed();
    run_poll_loop(client, &staged, interval, timeout, elapsed, on_wait).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::signer_client::SignerRequest;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    struct StubServer {
        _dir: TempDir,
        sock_path: PathBuf,
        recorded: Arc<Mutex<Vec<SignerRequest>>>,
        handle: tokio::task::JoinHandle<()>,
    }

    impl Drop for StubServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    async fn spawn_stub(responses: Vec<serde_json::Value>) -> StubServer {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("stub.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let rec_clone = recorded.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let queue = queue.clone();
                let rec = rec_clone.clone();
                tokio::spawn(async move {
                    let mut reader = BufReader::new(&mut stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    if let Ok(req) = serde_json::from_str::<SignerRequest>(line.trim()) {
                        rec.lock().unwrap().push(req);
                    }
                    let next = queue.lock().unwrap().pop_front().unwrap_or_else(|| {
                        serde_json::json!({"ok": false, "code": "unexpected_call", "message": "stub queue empty"})
                    });
                    let payload = format!("{next}\n");
                    let _ = stream.write_all(payload.as_bytes()).await;
                });
            }
        });
        StubServer {
            _dir: dir,
            sock_path,
            recorded,
            handle,
        }
    }

    fn sample_policy() -> serde_json::Value {
        serde_json::json!({"daily_cap_pct": 25})
    }

    #[tokio::test]
    async fn stage_maps_step_up_required_pending_and_policy_errors() {
        let stub = spawn_stub(vec![
            serde_json::json!({
                "ok": false,
                "code": "step_up_required",
                "message": "run: sudo vaughan signerd approve",
                "policy_digest": "digest_abc",
                "pending": {
                    "policy_digest": "digest_abc",
                    "expires_at_unix": 1700000600u64,
                    "approval_present": false
                }
            }),
            serde_json::json!({
                "ok": false,
                "code": "step_up_pending",
                "message": "a staged policy update is already pending approval"
            }),
            serde_json::json!({
                "ok": false,
                "code": "policy_invalid",
                "message": "invalid daily_cap_pct"
            }),
        ])
        .await;
        let client = SignerClient::new(&stub.sock_path);

        let staged = stage(&client, &sample_policy(), Some("old_1".into()))
            .await
            .unwrap();
        assert_eq!(staged.policy_digest, "digest_abc");
        assert_eq!(staged.expires_at_unix, 1700000600);
        assert_eq!(staged.expected_digest.as_deref(), Some("old_1"));

        let err_pending = stage(&client, &sample_policy(), None).await.unwrap_err();
        assert_eq!(
            err_pending,
            StepUpError::Daemon {
                code: "step_up_pending".into(),
                message: "a staged policy update is already pending approval".into(),
            }
        );

        let err_invalid = stage(&client, &sample_policy(), None).await.unwrap_err();
        assert_eq!(
            err_invalid,
            StepUpError::Daemon {
                code: "policy_invalid".into(),
                message: "invalid daily_cap_pct".into(),
            }
        );
    }

    fn stub_stage_wait_ready_apply() -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({
                "ok": false,
                "code": "step_up_required",
                "message": "run: sudo vaughan signerd approve",
                "policy_digest": "digest_ok"
            }),
            serde_json::json!({
                "ok": true,
                "pending": {
                    "policy_digest": "digest_ok",
                    "expires_at_unix": 1700000600u64,
                    "approval_present": false
                }
            }),
            serde_json::json!({
                "ok": true,
                "pending": {
                    "policy_digest": "digest_ok",
                    "expires_at_unix": 1700000600u64,
                    "approval_present": true
                }
            }),
            serde_json::json!({"ok": true, "policy_digest": "digest_ok"}),
        ]
    }

    #[tokio::test]
    async fn poll_waiting_then_ready_then_apply_succeeds_and_sends_client_draft() {
        let draft = sample_policy();
        let stub = spawn_stub(stub_stage_wait_ready_apply()).await;
        let client = SignerClient::new(&stub.sock_path);

        let mut tick = 1000u64;
        let mut wait_calls = 0usize;
        let new_digest = run_to_completion_with_clock(
            &client,
            draft.clone(),
            Some("old_0".into()),
            Duration::ZERO,
            Duration::from_secs(60),
            || {
                let v = tick;
                tick += 5;
                v
            },
            |_staged, state| {
                wait_calls += 1;
                assert_eq!(
                    *state,
                    StepUpState::Waiting {
                        expires_at_unix: 1700000600
                    }
                );
            },
        )
        .await
        .unwrap();

        assert_eq!(new_digest, "digest_ok");
        assert_eq!(wait_calls, 1);

        let ops: Vec<SignerRequest> = stub.recorded.lock().unwrap().clone();
        assert_eq!(ops.len(), 4);
        assert_eq!(ops[0].op, "policy_update");
        assert_eq!(ops[0].auth.as_deref(), Some("stage"));
        assert_eq!(ops[1].op, "policy_pending");
        assert_eq!(ops[2].op, "policy_pending");
        assert_eq!(ops[3].op, "policy_update");
        assert_eq!(ops[3].auth.as_deref(), Some("approved"));
        assert_eq!(ops[3].policy.as_ref(), Some(&draft));
        assert_eq!(ops[3].expected_digest.as_deref(), Some("old_0"));
    }

    #[tokio::test]
    async fn poll_returns_expired_when_daemon_reports_pending_null() {
        let stub = spawn_stub(vec![serde_json::json!({"ok": true, "pending": null})]).await;
        let client = SignerClient::new(&stub.sock_path);
        let staged = Staged {
            policy: sample_policy(),
            expected_digest: None,
            policy_digest: "digest_abc".into(),
            expires_at_unix: 1700000600,
        };
        let state = poll(&client, &staged).await.unwrap();
        assert_eq!(state, StepUpState::Expired);
    }

    #[tokio::test]
    async fn run_to_completion_with_clock_times_out_deterministically() {
        let stub = spawn_stub(vec![
            serde_json::json!({
                "ok": false,
                "code": "step_up_required",
                "message": "run: sudo vaughan signerd approve",
                "policy_digest": "digest_wait"
            }),
            serde_json::json!({
                "ok": true,
                "pending": {
                    "policy_digest": "digest_wait",
                    "expires_at_unix": 1700000600u64,
                    "approval_present": false
                }
            }),
            serde_json::json!({
                "ok": true,
                "pending": {
                    "policy_digest": "digest_wait",
                    "expires_at_unix": 1700000600u64,
                    "approval_present": false
                }
            }),
        ])
        .await;
        let client = SignerClient::new(&stub.sock_path);

        let mut now = 1000u64;
        let mut polls = 0usize;
        let err = run_to_completion_with_clock(
            &client,
            sample_policy(),
            None,
            Duration::ZERO,
            Duration::from_secs(20),
            || {
                let current = now;
                now += 15;
                current
            },
            |_, _| {
                polls += 1;
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err, StepUpError::TimedOut);
        assert_eq!(polls, 2);
    }

    #[tokio::test]
    async fn apply_maps_step_up_failed_to_not_approved() {
        let stub = spawn_stub(vec![serde_json::json!({
            "ok": false,
            "code": "step_up_failed",
            "message": "policy approval verification failed"
        })])
        .await;
        let client = SignerClient::new(&stub.sock_path);
        let staged = Staged {
            policy: sample_policy(),
            expected_digest: None,
            policy_digest: "digest_abc".into(),
            expires_at_unix: 1700000600,
        };
        let err = apply(&client, &staged).await.unwrap_err();
        assert_eq!(err, StepUpError::NotApproved);
    }

    #[tokio::test]
    async fn missing_socket_maps_to_transport_error() {
        let dir = tempfile::tempdir().unwrap();
        let client = SignerClient::new(dir.path().join("does-not-exist.sock"));
        let err = stage(&client, &sample_policy(), None).await.unwrap_err();
        assert!(matches!(err, StepUpError::Transport(ref s) if s.contains("signer_unavailable")));
    }
}
