//! Security checks for sentient guard files (mode 0600 and current process UID ownership).

use std::fs;
use std::path::Path;

use crate::error::AgentError;

/// Assert that a file exists, has mode 0600 (owner read/write only), and is owned
/// by the current process UID on Unix platforms. Fails closed with an actionable error.
pub fn assert_owner_only_file(path: &Path) -> Result<(), AgentError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::metadata(path).map_err(|e| {
            AgentError::SecurityViolation(format!(
                "failed to read metadata for {}: {e}",
                path.display()
            ))
        })?;

        let mode = meta.mode() & 0o777;
        if mode != 0o600 {
            return Err(AgentError::SecurityViolation(format!(
                "insecure file mode {:04o} on {} (expected 0600) — fix with `chmod 0600 {}`",
                mode,
                path.display(),
                path.display(),
            )));
        }

        let file_uid = meta.uid();
        let current_uid = rustix::process::getuid().as_raw();
        if file_uid != current_uid {
            return Err(AgentError::SecurityViolation(format!(
                "insecure file ownership on {}: owned by UID {}, but process runs as UID {} — fix with `chown {} {}`",
                path.display(),
                file_uid,
                current_uid,
                current_uid,
                path.display(),
            )));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn test_mode_0644_refused_mode_0600_accepted() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.file");
        fs::write(&file, b"secret").unwrap();

        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let err = assert_owner_only_file(&file).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("0644") && msg.contains("0600"));

        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(assert_owner_only_file(&file).is_ok());
    }
}
