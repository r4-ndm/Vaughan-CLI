//! Tests for `vaughan signerd migrate` and signer startup validation.

use std::fs;
use tempfile::tempdir;

use vaughan_cli::signerd::migrate::{
    check_legacy_policy_absent, copy_sync_zeroize_unlink_file, current_rfc3339, run_migrate,
    MIGRATED_FILENAME,
};

#[test]
fn test_current_rfc3339_format() {
    let ts = current_rfc3339();
    // Must be in format YYYY-MM-DDTHH:MM:SSZ
    assert_eq!(ts.len(), 20);
    assert_eq!(&ts[4..5], "-");
    assert_eq!(&ts[7..8], "-");
    assert_eq!(&ts[10..11], "T");
    assert_eq!(&ts[13..14], ":");
    assert_eq!(&ts[16..17], ":");
    assert_eq!(&ts[19..20], "Z");
}

#[test]
fn test_migrate_renames_all_files_writes_migrated_and_deletes_mcp_session() {
    let tmp = tempdir().unwrap();
    let old_dir = tmp.path().join("old_profile");
    let store_dir = tmp.path().join("store_sentient");

    fs::create_dir_all(&old_dir).unwrap();
    let proposals_old = old_dir.join("proposals");
    fs::create_dir_all(&proposals_old).unwrap();

    // Populate old profile files
    let wallet_bytes = b"{\"version\":1,\"active_network\":\"pulsechain\"}";
    let policy_bytes = b"max_slippage_bps = 50\n";
    let spend_bytes = b"{\"spent\":0}\n";
    let stealth_bytes = b"{\"pending\":[]}\n";
    let dca_bytes = b"{\"plans\":[]}\n";
    let proposal_bytes = b"{\"proposal_id\":\"prop-1\"}\n";
    let session_bytes = b"session_secret_token_12345\n";

    fs::write(old_dir.join("wallet.json"), wallet_bytes).unwrap();
    fs::write(old_dir.join("sentient-policy.toml"), policy_bytes).unwrap();
    fs::write(old_dir.join("sentient-spend.json"), spend_bytes).unwrap();
    fs::write(old_dir.join("stealth-pending.json"), stealth_bytes).unwrap();
    fs::write(old_dir.join("dca-plans.json"), dca_bytes).unwrap();
    fs::write(proposals_old.join("p1.json"), proposal_bytes).unwrap();
    fs::write(old_dir.join("mcp.session"), session_bytes).unwrap();

    // Run migration
    run_migrate("sentient", Some(&store_dir), Some(&old_dir)).expect("migrate should succeed");

    // 1. Files exist in store directory with identical content
    assert_eq!(
        fs::read(store_dir.join("wallet.json")).unwrap(),
        wallet_bytes
    );
    assert_eq!(
        fs::read(store_dir.join("sentient-policy.toml")).unwrap(),
        policy_bytes
    );
    assert_eq!(
        fs::read(store_dir.join("sentient-spend.json")).unwrap(),
        spend_bytes
    );
    assert_eq!(
        fs::read(store_dir.join("stealth-pending.json")).unwrap(),
        stealth_bytes
    );
    assert_eq!(
        fs::read(store_dir.join("dca-plans.json")).unwrap(),
        dca_bytes
    );
    assert_eq!(
        fs::read(store_dir.join("proposals").join("p1.json")).unwrap(),
        proposal_bytes
    );

    // 2. Renamed, not copied: files DO NOT exist in source directory (no duplicates)
    assert!(!old_dir.join("wallet.json").exists());
    assert!(!old_dir.join("sentient-policy.toml").exists());
    assert!(!old_dir.join("sentient-spend.json").exists());
    assert!(!old_dir.join("stealth-pending.json").exists());
    assert!(!old_dir.join("dca-plans.json").exists());
    assert!(!old_dir.join("proposals").exists());

    // 3. mcp.session deleted
    assert!(!old_dir.join("mcp.session").exists());

    // 4. MIGRATED file written with old path and timestamp
    let migrated_path = store_dir.join(MIGRATED_FILENAME);
    assert!(migrated_path.exists());
    let migrated_content = fs::read_to_string(&migrated_path).unwrap();
    assert!(migrated_content.contains(&format!("old_path: {}", old_dir.display())));
    assert!(migrated_content.contains("timestamp: "));

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mode = fs::metadata(&store_dir).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o700);

        let wallet_mode = fs::metadata(store_dir.join("wallet.json")).unwrap().mode() & 0o777;
        assert_eq!(wallet_mode, 0o600);

        let policy_mode = fs::metadata(store_dir.join("sentient-policy.toml"))
            .unwrap()
            .mode()
            & 0o777;
        assert_eq!(policy_mode, 0o600);
    }
}

#[test]
fn test_migrate_twice_is_idempotent() {
    let tmp = tempdir().unwrap();
    let old_dir = tmp.path().join("old_profile");
    let store_dir = tmp.path().join("store_sentient");

    fs::create_dir_all(&old_dir).unwrap();
    fs::write(old_dir.join("wallet.json"), b"wallet-data").unwrap();
    fs::write(old_dir.join("sentient-policy.toml"), b"policy-data").unwrap();

    // First migration
    run_migrate("sentient", Some(&store_dir), Some(&old_dir)).expect("first migration succeeds");
    assert!(store_dir.join("wallet.json").exists());
    assert!(store_dir.join(MIGRATED_FILENAME).exists());

    // Second migration: should report already migrated and exit Ok(()) without modifying data
    let res = run_migrate("sentient", Some(&store_dir), Some(&old_dir));
    assert!(res.is_ok(), "second migration must succeed idempotently");
    assert_eq!(
        fs::read(store_dir.join("wallet.json")).unwrap(),
        b"wallet-data"
    );
}

#[test]
fn test_copy_sync_zeroize_unlink_leaves_no_plaintext_source() {
    let tmp = tempdir().unwrap();
    let src = tmp.path().join("src.txt");
    let dst = tmp.path().join("dst.txt");

    let secret = b"my_super_secret_plaintext";
    fs::write(&src, secret).unwrap();

    copy_sync_zeroize_unlink_file(&src, &dst).expect("copy zeroize unlink succeeds");

    // Destination has the secret
    assert_eq!(fs::read(&dst).unwrap(), secret);
    // Source was unlinked
    assert!(!src.exists());
}

#[test]
fn test_signerd_refuses_to_start_when_old_policy_exists() {
    let tmp = tempdir().unwrap();
    let old_dir = tmp.path().join("old_profile");
    let store_dir = tmp.path().join("store");

    fs::create_dir_all(&old_dir).unwrap();
    fs::create_dir_all(&store_dir).unwrap();

    let old_policy = old_dir.join("sentient-policy.toml");
    let store_policy = store_dir.join("sentient-policy.toml");

    fs::write(&old_policy, b"old_policy_content").unwrap();
    fs::write(&store_policy, b"store_policy_content").unwrap();

    // With old policy present alongside store policy: check_legacy_policy_absent fails
    let err = check_legacy_policy_absent(&store_dir, Some(&old_dir)).unwrap_err();
    let msg = err.to_string();

    assert!(msg.contains("refusing to start signerd"));
    assert!(msg.contains("legacy policy file still exists at"));
    assert!(msg.contains(&old_policy.display().to_string()));
    assert!(msg.contains(&store_policy.display().to_string()));
    assert!(msg.contains("a second copy is a second source of truth"));

    // Once the old policy is removed, starting check succeeds
    fs::remove_file(&old_policy).unwrap();
    assert!(check_legacy_policy_absent(&store_dir, Some(&old_dir)).is_ok());
}
