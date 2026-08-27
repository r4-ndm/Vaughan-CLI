//! Live end-to-end: Freedom Browser's real `transport.js` (Node) against a
//! real Vaughan `ProviderServer` with the session-token gate (required for
//! every origin by default).
//!
//! Proves the parity PR contract both ways:
//!   * token file under `$XDG_DATA_HOME/vaughan-cli/provider.session` →
//!     `Authorization: Bearer` → gate accepts → `eth_requestAccounts` answers
//!   * a bogus `VAUGHAN_PROVIDER_SESSION_TOKEN` override → gate rejects
//!
//! Requires Node and a Freedom Browser checkout. Skips unless `FREEDOM_REPO`
//! points at it (e.g. `FREEDOM_REPO=~/Desktop/freedom-browser`).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use vaughan_core::core::ProviderSessionToken;
use vaughan_provider::{Eip1193Handler, ProviderError};
use vaughan_tui::provider::{HostRequest, ProviderHost};

const FREEDOM_ORIGIN: &str = "https://freedom.browser";
/// Anvil dev account #0 — clearly test-only, never a real key.
const TEST_ADDRESS: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

const NODE_SCRIPT: &str = r#"
const path = require('path');
const { rpcRequest } = require(path.join(process.env.FREEDOM_REPO, 'src/main/wallet/vaughan/transport'));
(async () => {
  try {
    const accounts = await rpcRequest('eth_requestAccounts', [], { timeoutMs: 3000 });
    console.log('OK', Array.isArray(accounts) ? accounts.join(',') : JSON.stringify(accounts));
  } catch (err) {
    console.error('FAIL', err.code || '', err.message);
    process.exit(1);
  }
})();
"#;

fn freedom_repo() -> Option<String> {
    let repo = std::env::var("FREEDOM_REPO").ok()?;
    let transport = Path::new(&repo).join("src/main/wallet/vaughan/transport.js");
    transport.exists().then_some(repo)
}

async fn spawn_token_gated_provider(
    token: &ProviderSessionToken,
) -> (tokio::task::JoinHandle<Result<(), ProviderError>>, String) {
    let (requests, mut rx) = mpsc::channel::<HostRequest>(16);
    let host = ProviderHost::new(requests);
    let handler = Eip1193Handler::new(Arc::new(host));
    let server = vaughan_provider::ProviderServer::bind(0)
        .await
        .unwrap()
        .with_trusted_origins([FREEDOM_ORIGIN])
        .unwrap()
        .with_session_token(token.as_str());
    let url = server.url();
    let events = vaughan_provider::EventBus::new();
    let task = tokio::spawn(server.serve(Arc::new(handler), events));

    tokio::spawn(async move {
        while let Some(request) = rx.recv().await {
            match request {
                HostRequest::RequestAccounts { reply, .. } => {
                    let _ = reply.send(Ok(vec![TEST_ADDRESS.to_string()]));
                }
                HostRequest::Accounts { reply, .. } => {
                    let _ = reply.send(Ok(vec![TEST_ADDRESS.to_string()]));
                }
                HostRequest::ChainId { reply } => {
                    let _ = reply.send(Ok("0x3af".to_string()));
                }
                HostRequest::SwitchChain { reply, .. } => {
                    let _ = reply.send(Err(ProviderError::UnrecognizedChain(
                        "not supported in e2e".to_string(),
                    )));
                }
                HostRequest::Approval { reply, .. } => {
                    let _ = reply.send(Err(ProviderError::Internal(
                        "approvals not supported in e2e".to_string(),
                    )));
                }
                HostRequest::RpcRead { reply, .. } => {
                    let _ = reply.send(Err(ProviderError::Internal(
                        "read proxy not supported in e2e".to_string(),
                    )));
                }
            }
        }
    });

    (task, url)
}

fn run_freedom_transport(
    repo: &str,
    data_dir: &Path,
    url: &str,
    token_override: Option<&str>,
) -> std::process::Output {
    let mut cmd = std::process::Command::new("node");
    cmd.arg("-e")
        .arg(NODE_SCRIPT)
        .env("FREEDOM_REPO", repo)
        .env("FREEDOM_VAUGHAN_WS_URL", url)
        .env("XDG_DATA_HOME", data_dir)
        .env_remove("VAUGHAN_PROVIDER_SESSION_TOKEN");
    if let Some(token) = token_override {
        cmd.env("VAUGHAN_PROVIDER_SESSION_TOKEN", token);
    }
    cmd.output().expect("spawn node")
}

#[tokio::test(flavor = "multi_thread")]
async fn freedom_transport_session_token_end_to_end() {
    let Some(repo) = freedom_repo() else {
        eprintln!("skipping: set FREEDOM_REPO to a Freedom Browser checkout");
        return;
    };
    let data_dir = tempfile::tempdir().unwrap();
    let profile_dir = data_dir.path().join("vaughan-cli");
    let token = ProviderSessionToken::generate();
    token.write(&profile_dir).unwrap();

    let (task, url) = spawn_token_gated_provider(&token).await;

    // Positive: token resolved from $XDG_DATA_HOME/vaughan-cli/provider.session
    // and presented as a Bearer header; the gate accepts.
    let out = tokio::task::spawn_blocking({
        let repo = repo.clone();
        let dir = data_dir.path().to_path_buf();
        let url = url.clone();
        move || run_freedom_transport(&repo, &dir, &url, None)
    })
    .await
    .unwrap();
    assert!(
        out.status.success(),
        "freedom transport failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("OK {TEST_ADDRESS}")),
        "unexpected stdout: {stdout}"
    );

    // Negative: bogus env override must be rejected by the gate.
    let out = tokio::task::spawn_blocking({
        let dir = data_dir.path().to_path_buf();
        let url = url.clone();
        move || run_freedom_transport(&repo, &dir, &url, Some("deadbeef"))
    })
    .await
    .unwrap();
    assert!(
        !out.status.success(),
        "bogus token must not connect: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("VAUGHAN_DISCONNECTED"),
        "expected fail-fast VAUGHAN_DISCONNECTED on gate drop, got: {stderr}"
    );

    drop(data_dir);
    task.abort();
    let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
}
