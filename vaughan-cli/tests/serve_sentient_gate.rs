//! Integration tests: `vaughan serve --profile sentient` operates as a thin client.
//!
//! Verifies:
//! 1. When the signer daemon is unreachable, `vaughan serve --profile sentient` exits non-zero with actionable error.
//! 2. When running against a signer daemon, `vaughan serve --profile sentient` creates NO `mcp.session` token file.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::tempdir;

fn file_exists_recursive(dir: &Path, target_name: &str) -> bool {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if file_exists_recursive(&p, target_name) {
                    return true;
                }
            } else if p.file_name().and_then(|s| s.to_str()) == Some(target_name) {
                return true;
            }
        }
    }
    false
}

#[test]
fn test_serve_sentient_unreachable_socket_exits_nonzero() {
    let bin = env!("CARGO_BIN_EXE_vaughan");
    let output = Command::new(bin)
        .args(["--profile", "sentient", "serve"])
        .env(
            "VAUGHAN_SIGNER_SOCKET",
            "/tmp/nonexistent_signer_socket_9999.sock",
        )
        .output()
        .expect("failed to execute vaughan binary");

    assert!(
        !output.status.success(),
        "must exit non-zero when signer daemon is unreachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("signer daemon unreachable")
            && stderr.contains("ensure vaughan-signerd is active"),
        "expected actionable error on stderr, got:\n{stderr}"
    );
}

#[tokio::test]
async fn test_serve_sentient_creates_no_mcp_session_file() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("mock_signerd.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();

    let server_task = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        while let Ok((mut stream, _)) = listener.accept().await {
            let resp = serde_json::json!({
                "ok": true,
                "address": "0x1111111111111111111111111111111111111111",
                "chain_id": 943
            });
            let _ = stream.write_all((resp.to_string() + "\n").as_bytes()).await;
        }
    });

    let bin = env!("CARGO_BIN_EXE_vaughan");
    let mut child = Command::new(bin)
        .args(["--profile", "sentient", "serve"])
        .env("HOME", dir.path())
        .env("VAUGHAN_SIGNER_SOCKET", &socket_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn vaughan binary");

    tokio::time::sleep(Duration::from_millis(1000)).await;

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        !file_exists_recursive(dir.path(), "mcp.session"),
        "mcp.session must NEVER be created for a sentient profile"
    );

    server_task.abort();
}
