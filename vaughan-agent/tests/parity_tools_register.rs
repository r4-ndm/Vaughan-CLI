//! New parity tools appear in the assist registry (names only; no RPC).

use std::path::PathBuf;

use vaughan_agent::tools::{default_assist_registry, default_assist_registry_for};

#[test]
fn assist_registry_includes_parity_verbs() {
    let registry = default_assist_registry();
    let names: Vec<_> = registry.definitions().into_iter().map(|d| d.name).collect();
    for required in [
        "propose_approve",
        "propose_v3_increase",
        "propose_v3_decrease",
        "propose_v3_collect",
        "quote_bridge",
        "propose_bridge",
        "list_transfers",
        "resolve_token",
        "watch_balance",
        "propose_stealth_send",
        "propose_batch_7702",
    ] {
        assert!(
            names.iter().any(|n| n == required),
            "missing tool: {required}"
        );
    }
    assert!(!names.iter().any(|n| n == "import_token"));
}

#[test]
fn assist_registry_with_profile_includes_import_token() {
    let dir = PathBuf::from("/tmp/vaughan-test-profile");
    let registry = default_assist_registry_for(Some(&dir));
    let names: Vec<_> = registry.definitions().into_iter().map(|d| d.name).collect();
    assert!(names.iter().any(|n| n == "import_token"));
}
