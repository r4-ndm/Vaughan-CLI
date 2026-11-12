//! Integration test: `vaughan serve --profile sentient` requires VAUGHAN_SIGNER_STORE.

use std::process::Command;

#[test]
fn test_serve_sentient_without_env_var_exits_nonzero() {
    let bin = env!("CARGO_BIN_EXE_vaughan");
    let output = Command::new(bin)
        .args(["--profile", "sentient", "serve", "--password-env", "DUMMY"])
        .env("DUMMY", "password123")
        .env_remove("VAUGHAN_SIGNER_STORE")
        .output()
        .expect("failed to execute vaughan binary");

    assert!(
        !output.status.success(),
        "must exit non-zero without VAUGHAN_SIGNER_STORE"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("VAUGHAN_SIGNER_STORE is required")
            && stderr.contains("export VAUGHAN_SIGNER_STORE="),
        "expected actionable error on stderr, got:\n{stderr}"
    );
}

#[test]
fn test_serve_sentient_with_nonexistent_store_exits_nonzero() {
    let bin = env!("CARGO_BIN_EXE_vaughan");
    let output = Command::new(bin)
        .args(["--profile", "sentient", "serve", "--password-env", "DUMMY"])
        .env("DUMMY", "password123")
        .env(
            "VAUGHAN_SIGNER_STORE",
            "/tmp/nonexistent_signer_store_dir_9999",
        )
        .output()
        .expect("failed to execute vaughan binary");

    assert!(
        !output.status.success(),
        "must exit non-zero with nonexistent store"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not exist"),
        "expected does not exist error on stderr, got:\n{stderr}"
    );
}
