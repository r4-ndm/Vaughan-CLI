//! Migration helper for moving profile data into the signer-owned store (`vaughan signerd migrate`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use vaughan_core::core::persistence::StateManager;

pub const MIGRATED_FILENAME: &str = "MIGRATED";

/// Files to migrate from legacy profile directory.
pub const MIGRATION_FILES: &[&str] = &[
    "wallet.json",
    "sentient-policy.toml",
    "sentient-spend.json",
    "stealth-pending.json",
    "dca-plans.json",
];

/// Format current time as RFC3339 in UTC without external crates.
pub fn current_rfc3339() -> String {
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = duration.as_secs();

    let days = total_secs / 86400;
    let rem_secs = total_secs % 86400;
    let hours = rem_secs / 3600;
    let rem_secs = rem_secs % 3600;
    let minutes = rem_secs / 60;
    let seconds = rem_secs % 60;

    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Assert that the legacy `sentient-policy.toml` does not exist alongside the signer store policy.
///
/// Refuses to start `signerd` if a second policy file is found, naming both paths with an actionable error.
pub fn check_legacy_policy_absent(
    store: &Path,
    old_profile_dir: Option<&Path>,
) -> anyhow::Result<()> {
    let legacy_dir = match old_profile_dir {
        Some(explicit) => explicit.to_path_buf(),
        None => StateManager::profile_path("sentient")
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from(".")),
    };

    let legacy_policy = legacy_dir.join("sentient-policy.toml");
    let store_policy = store.join("sentient-policy.toml");

    if legacy_policy.exists() {
        let is_same = match (legacy_policy.canonicalize(), store_policy.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => legacy_policy == store_policy,
        };
        if !is_same {
            anyhow::bail!(
                "refusing to start signerd: legacy policy file still exists at `{}` alongside store policy at `{}` — a second copy is a second source of truth; remove the legacy copy to proceed",
                legacy_policy.display(),
                store_policy.display()
            );
        }
    }

    Ok(())
}

/// Run migration for the given profile.
pub fn run_migrate(
    profile: &str,
    store_root_override: Option<&Path>,
    old_profile_dir_override: Option<&Path>,
) -> anyhow::Result<()> {
    // 1. Determine old profile directory
    let old_dir = match old_profile_dir_override {
        Some(explicit) => explicit.to_path_buf(),
        None => {
            let wallet_path = StateManager::profile_path(profile)
                .map_err(|e| anyhow::anyhow!("could not resolve source profile path: {e}"))?;
            wallet_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        }
    };

    // 2. Determine target store directory
    let target_dir = match store_root_override {
        Some(explicit) => explicit.to_path_buf(),
        None => PathBuf::from(format!("/var/lib/vaughan/{profile}")),
    };

    // 3. Ownership / Invoking UID check
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let invoking_uid = rustix::process::getuid().as_raw();

        if target_dir.exists() {
            let meta = fs::metadata(&target_dir)?;
            let owner_uid = meta.uid();
            if owner_uid != invoking_uid {
                anyhow::bail!(
                    "cannot migrate: target store directory `{}` is owned by UID {}, but migrate was invoked as UID {} — run `vaughan signerd migrate` as the signer user (or chown {} {})",
                    target_dir.display(),
                    owner_uid,
                    invoking_uid,
                    invoking_uid,
                    target_dir.display()
                );
            }
        }
    }

    // 4. Create target directory at 0700 if not exists
    fs::create_dir_all(&target_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target_dir, fs::Permissions::from_mode(0o700))?;
    }

    // 5. Idempotency: if MIGRATED already exists, exit 0
    let migrated_file = target_dir.join(MIGRATED_FILENAME);
    if migrated_file.exists() {
        println!(
            "Profile '{profile}' already migrated to {}",
            target_dir.display()
        );
        return Ok(());
    }

    // Check if there is anything to migrate
    if !old_dir.exists() {
        anyhow::bail!(
            "cannot migrate: source profile directory `{}` does not exist",
            old_dir.display()
        );
    }

    // 6. Rename, not copy
    for filename in MIGRATION_FILES {
        let src = old_dir.join(filename);
        let dst = target_dir.join(filename);
        if src.exists() {
            migrate_file_rename_or_exdev(&src, &dst)?;
        }
    }

    // Proposals tree
    let proposals_src = old_dir.join("proposals");
    let proposals_dst = target_dir.join("proposals");
    if proposals_src.exists() {
        migrate_dir_rename_or_exdev(&proposals_src, &proposals_dst)?;
    }

    // 7. Delete <old>/mcp.session
    let old_session = old_dir.join("mcp.session");
    if old_session.exists() {
        let _ = fs::remove_file(&old_session);
    }

    // 8. Write MIGRATED
    let timestamp = current_rfc3339();
    let migrated_content = format!(
        "old_path: {}\ntimestamp: {}\n",
        old_dir.display(),
        timestamp
    );
    fs::write(&migrated_file, migrated_content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&migrated_file, fs::Permissions::from_mode(0o600));
    }

    // 9. Output manual step
    println!(
        "Migration complete: profile '{profile}' migrated to {}",
        target_dir.display()
    );
    println!();
    println!("Next manual step:");
    println!(
        "  Move the vault password from ~/.config/vaughan/serve.env into /etc/vaughan/sentient.env"
    );
    println!(
        "  (the installer scripts/install-signerd.sh does this automatically when run as root)."
    );

    Ok(())
}

fn is_exdev_error(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(rustix::io::Errno::XDEV.raw_os_error())
    }
    #[cfg(not(unix))]
    {
        let _ = e;
        false
    }
}

pub fn migrate_file_rename_or_exdev(src: &Path, dst: &Path) -> anyhow::Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(dst, fs::Permissions::from_mode(0o600));
            }
            Ok(())
        }
        Err(e) if is_exdev_error(&e) => {
            println!(
                "EXDEV across filesystems for {}: copied, synced, zeroized and unlinked source",
                src.display()
            );
            copy_sync_zeroize_unlink_file(src, dst)
        }
        Err(e) => Err(anyhow::anyhow!(
            "failed to rename {} to {}: {e}",
            src.display(),
            dst.display()
        )),
    }
}

pub fn copy_sync_zeroize_unlink_file(src: &Path, dst: &Path) -> anyhow::Result<()> {
    let data = fs::read(src)?;
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut dst_file = opts.open(dst)?;
        dst_file.write_all(&data)?;
        dst_file.sync_all()?;
    }
    // Zeroize source file
    {
        let mut src_file = fs::OpenOptions::new().write(true).open(src)?;
        let zeros = vec![0u8; data.len()];
        src_file.write_all(&zeros)?;
        src_file.sync_all()?;
    }
    // Unlink source
    fs::remove_file(src)?;
    Ok(())
}

pub fn migrate_dir_rename_or_exdev(src: &Path, dst: &Path) -> anyhow::Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(dst, fs::Permissions::from_mode(0o700));
            }
            Ok(())
        }
        Err(e) if is_exdev_error(&e) => {
            println!(
                "EXDEV across filesystems for directory {}: copied, synced, zeroized and unlinked source",
                src.display()
            );
            copy_and_zeroize_dir(src, dst)
        }
        Err(e) => Err(anyhow::anyhow!(
            "failed to rename {} to {}: {e}",
            src.display(),
            dst.display()
        )),
    }
}

pub fn copy_and_zeroize_dir(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dst, fs::Permissions::from_mode(0o700));
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let entry_src = entry.path();
        let entry_dst = dst.join(entry.file_name());
        if entry_src.is_dir() {
            copy_and_zeroize_dir(&entry_src, &entry_dst)?;
        } else {
            copy_sync_zeroize_unlink_file(&entry_src, &entry_dst)?;
        }
    }
    fs::remove_dir_all(src)?;
    Ok(())
}
