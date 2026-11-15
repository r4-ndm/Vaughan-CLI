//! Root-approved policy updates for `vaughan signerd` (S3b slice 1).
//!
//! # Security Model
//! When `vaughan-signerd` receives a `policy_update` with `"auth": "stage"`, it
//! validates the candidate policy and stages `policy-pending.json` (mode `0600`,
//! owned by `vaughan-signer`). A human operator running as root invokes
//! `vaughan signerd approve`, which reads both the current on-disk policy and
//! `policy-pending.json` directly from the store, renders the canonical field diff
//! and SHA-256 digests from those in-memory copies, prompts `[y/N]` on a TTY,
//! and writes `policy-approval.json` (`0440`, `root:vaughan-signer`).
//!
//! When `vaughan-signerd` receives `"auth": "approved"`, it opens
//! `policy-approval.json` with `O_RDONLY | O_NOFOLLOW`, validates `fstat` on that
//! single open descriptor (`st_uid == expected_uid`, `st_nlink == 1`,
//! `mode & 0o7777 == 0o440`, regular file), reads from the same descriptor, and
//! unlinks the file before parsing or applying (one-time use).

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use vaughan_agent::sentient::policy::{
    format_policy_diff_lines, LEGACY_DEGEN_POLICY_TOML, SENTIENT_POLICY_TOML,
};
use vaughan_agent::sentient::{AgentSessionPolicy, EnforcementMode};

/// Staged policy filename inside the signer store (mode `0600`, `vaughan-signer`).
pub const POLICY_PENDING_FILENAME: &str = "policy-pending.json";

/// Root-signed one-time approval filename inside the signer store (mode `0440`, `root:vaughan-signer`).
pub const POLICY_APPROVAL_FILENAME: &str = "policy-approval.json";

/// Expected owner UID of `policy-approval.json` in production (`root`).
pub const PRODUCTION_APPROVAL_UID: u32 = 0;

/// Default TTL for a minted root approval (seconds).
pub const DEFAULT_APPROVAL_TTL_SECS: u64 = 120;

/// TTL for a staged pending policy slot (10 minutes).
pub const PENDING_TTL_SECS: u64 = 600;

/// Canonical error code returned for every failed approval verification.
pub const STEP_UP_FAILED_CODE: &str = "step_up_failed";

/// Canonical error message returned for every failed approval verification.
pub const STEP_UP_FAILED_MSG: &str = "policy approval verification failed";

const MAX_APPROVAL_FILE_BYTES: u64 = 65_536;
const MAX_POLICY_FILE_BYTES: u64 = 262_144;

/// Root-signed one-time approval binding an exact `old_digest -> new_digest` transition.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub old_digest: String,
    pub new_digest: String,
    pub issued_at_unix: u64,
    pub expires_at_unix: u64,
}

/// Staged policy update awaiting root confirmation via `vaughan signerd approve`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PendingPolicy {
    pub policy: AgentSessionPolicy,
    pub staged_at_unix: u64,
    pub expires_at_unix: u64,
}

/// Errors returned by approval consumption or writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalError {
    Missing,
    Untrusted(String),
    Corrupt(String),
    Expired,
    UnlinkFailed(String),
    Io(String),
}

impl fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "approval file missing"),
            Self::Untrusted(msg) => write!(f, "untrusted approval file: {msg}"),
            Self::Corrupt(msg) => write!(f, "corrupt approval file: {msg}"),
            Self::Expired => write!(f, "approval expired"),
            Self::UnlinkFailed(msg) => write!(f, "failed to unlink approval file: {msg}"),
            Self::Io(msg) => write!(f, "approval I/O error: {msg}"),
        }
    }
}

impl std::error::Error for ApprovalError {}

/// Errors returned while staging `policy-pending.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingStageError {
    AlreadyPending,
    Io(String),
}

fn is_valid_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn open_nofollow_readonly(path: &Path) -> std::io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    opts.open(path)
}

fn read_bounded_fd(file: File, max_bytes: u64) -> Result<String, String> {
    let mut raw = String::new();
    let mut limited = file.take(max_bytes);
    limited
        .read_to_string(&mut raw)
        .map_err(|e| format!("read failed: {e}"))?;
    if raw.len() as u64 >= max_bytes {
        return Err("file exceeds maximum allowed size".into());
    }
    Ok(raw)
}

fn check_approval_metadata(meta: &fs::Metadata, expected_uid: u32) -> Result<(), ApprovalError> {
    if !meta.file_type().is_file() {
        return Err(ApprovalError::Untrusted("not a regular file".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != expected_uid {
            return Err(ApprovalError::Untrusted(format!(
                "owner uid {} != expected {}",
                meta.uid(),
                expected_uid
            )));
        }
        if meta.nlink() != 1 {
            return Err(ApprovalError::Untrusted(format!(
                "hardlink count {} != 1",
                meta.nlink()
            )));
        }
        let mode = meta.mode() & 0o7777;
        if mode != 0o0440 {
            return Err(ApprovalError::Untrusted(format!("mode {mode:04o} != 0440")));
        }
    }
    #[cfg(not(unix))]
    let _ = expected_uid;
    Ok(())
}

fn parse_and_validate_approval(raw: &str, now: u64) -> Result<Approval, ApprovalError> {
    let approval: Approval = serde_json::from_str(raw)
        .map_err(|e| ApprovalError::Corrupt(format!("invalid approval json: {e}")))?;

    if !is_valid_sha256_hex(&approval.old_digest)
        || !is_valid_sha256_hex(&approval.new_digest)
        || approval.issued_at_unix > approval.expires_at_unix
    {
        return Err(ApprovalError::Corrupt(
            "invalid digest format or timestamps".into(),
        ));
    }

    if now >= approval.expires_at_unix {
        return Err(ApprovalError::Expired);
    }

    Ok(approval)
}

/// Read-only trust verdict for `policy-approval.json` without consuming or modifying files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPresence {
    /// A valid, trusted, unexpired `policy-approval.json` is present.
    Present,
    /// `policy-approval.json` is missing, symlinked, untrusted, corrupt, or expired.
    Absent,
}

impl ApprovalPresence {
    /// Return `true` if a trusted unexpired approval file is present.
    pub fn is_present(self) -> bool {
        matches!(self, Self::Present)
    }
}

/// Read-only check of `policy-approval.json` at the current wall-clock time.
///
/// Uses [`inspect_approval_at`] with the same `uid`/`mode`/`nlink`/symlink and
/// payload validation rules as [`consume_approval`], without unlinking or modifying files.
pub fn inspect_approval(store: &Path, expected_uid: u32) -> ApprovalPresence {
    inspect_approval_at(store, expected_uid, vaughan_agent::dca::now_unix())
}

/// Read-only check of `policy-approval.json` at explicit timestamp `now`.
pub fn inspect_approval_at(store: &Path, expected_uid: u32, now: u64) -> ApprovalPresence {
    let path = store.join(POLICY_APPROVAL_FILENAME);
    let Ok(sym_meta) = fs::symlink_metadata(&path) else {
        return ApprovalPresence::Absent;
    };
    if sym_meta.file_type().is_symlink()
        || check_approval_metadata(&sym_meta, expected_uid).is_err()
    {
        return ApprovalPresence::Absent;
    }

    let Ok(file) = open_nofollow_readonly(&path) else {
        return ApprovalPresence::Absent;
    };
    let Ok(fd_meta) = file.metadata() else {
        return ApprovalPresence::Absent;
    };
    if check_approval_metadata(&fd_meta, expected_uid).is_err() {
        return ApprovalPresence::Absent;
    }

    let Ok(raw) = read_bounded_fd(file, MAX_APPROVAL_FILE_BYTES) else {
        return ApprovalPresence::Absent;
    };
    if parse_and_validate_approval(&raw, now).is_ok() {
        ApprovalPresence::Present
    } else {
        ApprovalPresence::Absent
    }
}

/// Open, `fstat`-verify, read, and atomically unlink `policy-approval.json` via one fd.
///
/// Trust rules (fail closed):
/// - Opened `O_RDONLY | O_NOFOLLOW` (symlinks fail).
/// - `fstat` on the same open fd confirms regular file, `st_uid == expected_uid`,
///   `st_nlink == 1`, and `(st_mode & 0o7777) == 0o0440`.
/// - Contents are read from that same open fd.
/// - File is unlinked BEFORE parsing/expiry checks (one-time use even on failure).
pub fn consume_approval(
    store: &Path,
    expected_uid: u32,
    now: u64,
) -> Result<Approval, ApprovalError> {
    let path = store.join(POLICY_APPROVAL_FILENAME);
    let file = match open_nofollow_readonly(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ApprovalError::Missing),
        Err(e) => {
            let _ = fs::remove_file(&path);
            return Err(ApprovalError::Untrusted(format!("open failed: {e}")));
        }
    };

    let meta = match file.metadata() {
        Ok(m) => m,
        Err(e) => {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(ApprovalError::Untrusted(format!("fstat failed: {e}")));
        }
    };
    if let Err(err) = check_approval_metadata(&meta, expected_uid) {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(err);
    }

    let raw = match read_bounded_fd(file, MAX_APPROVAL_FILE_BYTES) {
        Ok(s) => s,
        Err(msg) => {
            let _ = fs::remove_file(&path);
            return Err(ApprovalError::Io(msg));
        }
    };

    // One-time use: unlink BEFORE parsing, expiry checks, or applying.
    fs::remove_file(&path)
        .map_err(|e| ApprovalError::UnlinkFailed(format!("unlink failed: {e}")))?;

    parse_and_validate_approval(&raw, now)
}

/// Write `approval` atomically to `store/.policy-approval.json.tmp` at mode `0440`
/// and rename it to `store/policy-approval.json`.
///
/// Callers running as root (`euid == 0`) are responsible for `chown` to
/// `root:vaughan-signer`; non-root tests skip `chown`.
pub fn write_approval(store: &Path, approval: &Approval) -> Result<PathBuf, ApprovalError> {
    if !is_valid_sha256_hex(&approval.old_digest)
        || !is_valid_sha256_hex(&approval.new_digest)
        || approval.issued_at_unix > approval.expires_at_unix
    {
        return Err(ApprovalError::Corrupt(
            "refusing to write approval with invalid digests or timestamps".into(),
        ));
    }

    let body = serde_json::to_string_pretty(approval)
        .map_err(|e| ApprovalError::Corrupt(format!("serialize approval: {e}")))?;

    let tmp = store.join(format!(".{POLICY_APPROVAL_FILENAME}.tmp"));
    let dst = store.join(POLICY_APPROVAL_FILENAME);

    let _ = fs::remove_file(&tmp);
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o440);
        opts.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }

    let mut file = opts
        .open(&tmp)
        .map_err(|e| ApprovalError::Io(format!("create {}: {e}", tmp.display())))?;
    file.write_all(body.as_bytes())
        .map_err(|e| ApprovalError::Io(format!("write {}: {e}", tmp.display())))?;
    file.write_all(b"\n")
        .map_err(|e| ApprovalError::Io(format!("write {}: {e}", tmp.display())))?;
    file.sync_all()
        .map_err(|e| ApprovalError::Io(format!("sync {}: {e}", tmp.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o440))
            .map_err(|e| ApprovalError::Io(format!("chmod 0440 {}: {e}", tmp.display())))?;
    }
    drop(file);

    fs::rename(&tmp, &dst)
        .map_err(|e| ApprovalError::Io(format!("rename to {}: {e}", dst.display())))?;

    Ok(dst)
}

fn existing_pending_unexpired(path: &Path, now: u64) -> bool {
    let file = match open_nofollow_readonly(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match file.metadata() {
            Ok(m) => {
                if !m.file_type().is_file() || (m.mode() & 0o7777) != 0o0600 {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    let raw = match read_bounded_fd(file, MAX_POLICY_FILE_BYTES) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let existing = match serde_json::from_str::<PendingPolicy>(&raw) {
        Ok(p) => p,
        Err(_) => return false,
    };
    now < existing.expires_at_unix
}

/// Stage `policy-pending.json` (mode `0600`, TTL 10 min) if no unexpired pending exists.
pub fn stage_pending_policy(
    store: &Path,
    policy: &AgentSessionPolicy,
    now: u64,
) -> Result<(), PendingStageError> {
    stage_pending_policy_with_replace(store, policy, now, false)
}

/// Stage `policy-pending.json` (mode `0600`, TTL 10 min).
///
/// When an unexpired pending file already exists:
/// - `allow_replace == false` returns `PendingStageError::AlreadyPending` without touching disk;
/// - `allow_replace == true` atomically replaces the pending file and removes any existing
///   `policy-approval.json` so an approval minted for the superseded pending cannot remain.
pub fn stage_pending_policy_with_replace(
    store: &Path,
    policy: &AgentSessionPolicy,
    now: u64,
    allow_replace: bool,
) -> Result<(), PendingStageError> {
    let path = store.join(POLICY_PENDING_FILENAME);
    let had_unexpired = existing_pending_unexpired(&path, now);
    if had_unexpired && !allow_replace {
        return Err(PendingStageError::AlreadyPending);
    }

    let pending = PendingPolicy {
        policy: policy.clone(),
        staged_at_unix: now,
        expires_at_unix: now.saturating_add(PENDING_TTL_SECS),
    };
    let body = serde_json::to_string_pretty(&pending)
        .map_err(|e| PendingStageError::Io(format!("serialize pending policy: {e}")))?;

    let tmp = store.join(format!(".{POLICY_PENDING_FILENAME}.tmp"));
    let _ = fs::remove_file(&tmp);
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
        opts.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    let mut file = opts
        .open(&tmp)
        .map_err(|e| PendingStageError::Io(format!("create {}: {e}", tmp.display())))?;
    file.write_all(body.as_bytes())
        .map_err(|e| PendingStageError::Io(format!("write {}: {e}", tmp.display())))?;
    file.write_all(b"\n")
        .map_err(|e| PendingStageError::Io(format!("write {}: {e}", tmp.display())))?;
    file.sync_all()
        .map_err(|e| PendingStageError::Io(format!("sync {}: {e}", tmp.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| PendingStageError::Io(format!("chmod 0600 {}: {e}", tmp.display())))?;
    }
    drop(file);

    fs::rename(&tmp, &path)
        .map_err(|e| PendingStageError::Io(format!("rename to {}: {e}", path.display())))?;
    if had_unexpired && allow_replace {
        let _ = fs::remove_file(store.join(POLICY_APPROVAL_FILENAME));
    }
    Ok(())
}

/// Remove `policy-pending.json` if present.
pub fn clear_pending_policy(store: &Path) {
    let _ = fs::remove_file(store.join(POLICY_PENDING_FILENAME));
}

#[cfg(unix)]
fn find_group_gid(group_name: &str) -> Option<u32> {
    let content = fs::read_to_string("/etc/group").ok()?;
    for line in content.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 && parts[0] == group_name {
            return parts[2].parse::<u32>().ok();
        }
    }
    None
}

fn read_verified_open_file(
    file: File,
    path: &Path,
    expected_mode: u32,
    allowed_uids: &[u32],
    max_bytes: u64,
) -> anyhow::Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = file
            .metadata()
            .map_err(|e| anyhow::anyhow!("fstat {} failed: {e}", path.display()))?;
        if !meta.file_type().is_file() {
            anyhow::bail!("{} is not a regular file", path.display());
        }
        let mode = meta.mode() & 0o7777;
        if mode != expected_mode {
            anyhow::bail!(
                "{} has insecure mode {:04o} (expected {:04o})",
                path.display(),
                mode,
                expected_mode
            );
        }
        if !allowed_uids.contains(&meta.uid()) {
            anyhow::bail!(
                "{} has unexpected owner uid {} (allowed: {:?})",
                path.display(),
                meta.uid(),
                allowed_uids
            );
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (expected_mode, allowed_uids);
    }
    read_bounded_fd(file, max_bytes)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))
}

fn read_verified_store_file(
    path: &Path,
    expected_mode: u32,
    allowed_uids: &[u32],
    max_bytes: u64,
) -> anyhow::Result<String> {
    let file = open_nofollow_readonly(path)
        .map_err(|e| anyhow::anyhow!("cannot open {}: {e}", path.display()))?;
    read_verified_open_file(file, path, expected_mode, allowed_uids, max_bytes)
}

fn inspect_store_trust(store: &Path, effective_euid: u32) -> Result<(Vec<u32>, u32), String> {
    let store_meta = fs::symlink_metadata(store)
        .map_err(|e| format!("cannot inspect store {}: {e}", store.display()))?;
    if !store_meta.is_dir() {
        return Err(format!("store {} is not a directory", store.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mode = store_meta.mode() & 0o7777;
        if (mode & 0o022) != 0 {
            return Err(format!(
                "store {} has group/world-writable mode {:04o} (expected 0700 or 0750)",
                store.display(),
                mode
            ));
        }
        let mut uids = vec![store_meta.uid(), effective_euid];
        uids.sort_unstable();
        uids.dedup();
        Ok((uids, store_meta.gid()))
    }
    #[cfg(not(unix))]
    {
        let _ = effective_euid;
        Ok((vec![0u32], 0u32))
    }
}

/// Read and verify `policy-pending.json` without modifying `policy-approval.json`.
///
/// Uses the same trust checks as [`approve_with_io`] (`store` mode without group/world write,
/// `O_RDONLY | O_NOFOLLOW`, regular file, mode `0600`, `allowed_uids`, bounded read, and
/// non-expired timestamps).
pub fn load_pending_policy(store: &Path, now: u64) -> Result<PendingPolicy, ApprovalError> {
    #[cfg(unix)]
    let effective_euid = rustix::process::geteuid().as_raw();
    #[cfg(not(unix))]
    let effective_euid = 0u32;

    let (allowed_uids, _store_gid) =
        inspect_store_trust(store, effective_euid).map_err(ApprovalError::Untrusted)?;

    let pending_path = store.join(POLICY_PENDING_FILENAME);
    let pending_raw =
        read_verified_store_file(&pending_path, 0o0600, &allowed_uids, MAX_POLICY_FILE_BYTES)
            .map_err(|e| ApprovalError::Untrusted(format!("no valid staged policy: {e}")))?;
    let pending: PendingPolicy = serde_json::from_str(&pending_raw)
        .map_err(|e| ApprovalError::Corrupt(format!("invalid {}: {e}", pending_path.display())))?;
    if pending.staged_at_unix > pending.expires_at_unix {
        return Err(ApprovalError::Corrupt(
            "invalid pending policy timestamps".into(),
        ));
    }
    if now >= pending.expires_at_unix {
        return Err(ApprovalError::Expired);
    }
    Ok(pending)
}

fn load_current_policy_for_approve(
    store: &Path,
    allowed_uids: &[u32],
) -> anyhow::Result<AgentSessionPolicy> {
    for filename in [SENTIENT_POLICY_TOML, LEGACY_DEGEN_POLICY_TOML] {
        let path = store.join(filename);
        match open_nofollow_readonly(&path) {
            Ok(file) => {
                let raw = read_verified_open_file(
                    file,
                    &path,
                    0o0600,
                    allowed_uids,
                    MAX_POLICY_FILE_BYTES,
                )?;
                let policy: AgentSessionPolicy = toml::from_str(&raw)
                    .map_err(|e| anyhow::anyhow!("invalid {}: {e}", path.display()))?;
                policy.validate()?;
                return Ok(policy);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => anyhow::bail!("cannot open {}: {e}", path.display()),
        }
    }
    Ok(AgentSessionPolicy::default())
}

/// Interactive root approval command (`vaughan signerd approve --store <dir> [--ttl-secs 120]`).
pub fn run_approve(store: &Path, ttl_secs: u64) -> anyhow::Result<()> {
    #[cfg(unix)]
    let effective_euid = rustix::process::geteuid().as_raw();
    #[cfg(not(unix))]
    let effective_euid = 0u32;

    if effective_euid != PRODUCTION_APPROVAL_UID {
        anyhow::bail!(
            "vaughan signerd approve requires root (euid == 0, got {effective_euid}); run via sudo from a clean console or run0"
        );
    }

    let is_tty = std::io::stdin().is_terminal();
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    let now = vaughan_agent::dca::now_unix();

    let written = approve_with_io(
        store,
        ttl_secs,
        effective_euid,
        PRODUCTION_APPROVAL_UID,
        is_tty,
        now,
        &mut reader,
        &mut writer,
    )?;

    writeln!(writer, "approval written: {}", written.display())?;
    writer.flush()?;
    Ok(())
}

/// Core `approve` flow with injectable euid/TTY/clock/I/O for deterministic unit tests.
///
/// Reads `policy-pending.json` and the current on-disk policy directly from `store`
/// (never from argv/env/stdin), renders the diff and SHA-256 digests from those exact
/// in-memory copies, prompts `[y/N]`, and writes `policy-approval.json` (`0440`).
#[allow(clippy::too_many_arguments)]
pub fn approve_with_io<R: BufRead, W: Write>(
    store: &Path,
    ttl_secs: u64,
    effective_euid: u32,
    required_euid: u32,
    is_tty: bool,
    now: u64,
    reader: &mut R,
    writer: &mut W,
) -> anyhow::Result<PathBuf> {
    if effective_euid != required_euid {
        anyhow::bail!(
            "vaughan signerd approve requires root (euid == {required_euid}, got {effective_euid}); run from a clean console or run0"
        );
    }
    if !is_tty {
        anyhow::bail!(
            "vaughan signerd approve requires an interactive TTY on stdin (run from a clean console or run0)"
        );
    }
    if ttl_secs == 0 {
        anyhow::bail!("--ttl-secs must be >= 1");
    }

    let (allowed_uids, store_gid) =
        inspect_store_trust(store, effective_euid).map_err(anyhow::Error::msg)?;

    let pending_path = store.join(POLICY_PENDING_FILENAME);
    let pending_raw =
        read_verified_store_file(&pending_path, 0o0600, &allowed_uids, MAX_POLICY_FILE_BYTES)
            .map_err(|e| anyhow::anyhow!("no valid staged policy at store: {e}"))?;
    let pending: PendingPolicy = serde_json::from_str(&pending_raw)
        .map_err(|e| anyhow::anyhow!("invalid {}: {e}", pending_path.display()))?;
    if now >= pending.expires_at_unix {
        anyhow::bail!("staged policy in {} has expired", pending_path.display());
    }
    pending.policy.validate()?;

    let old_policy = load_current_policy_for_approve(store, &allowed_uids)?;
    if pending.policy.enforcement == EnforcementMode::Disabled
        && (!old_policy.acknowledge_unsafe || !pending.policy.acknowledge_unsafe)
    {
        anyhow::bail!(
            "staged policy sets enforcement=disabled without acknowledge_unsafe=true already persisted on disk"
        );
    }

    // Compute digests and diffs from the exact in-memory copies we display.
    let old_digest = old_policy.digest();
    let new_digest = pending.policy.digest();
    let diff_lines = format_policy_diff_lines(&old_policy, &pending.policy)
        .map_err(|e| anyhow::anyhow!("failed to compute policy diff: {e}"))?;

    writeln!(writer, "Store:      {}", store.display())?;
    writeln!(writer, "Old digest: {old_digest}")?;
    writeln!(writer, "New digest: {new_digest}")?;
    writeln!(writer, "TTL:        {ttl_secs}s")?;
    writeln!(writer, "Changes:")?;
    if diff_lines.is_empty() {
        writeln!(writer, "  (no field changes)")?;
    } else {
        for line in &diff_lines {
            writeln!(writer, "  {line}")?;
        }
    }
    write!(writer, "Approve this policy update? [y/N] ")?;
    writer.flush()?;

    let mut answer = String::new();
    if reader.read_line(&mut answer)? == 0 {
        anyhow::bail!("aborted: policy update not approved");
    }
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        anyhow::bail!("aborted: policy update not approved");
    }

    let approval = Approval {
        old_digest,
        new_digest,
        issued_at_unix: now,
        expires_at_unix: now.saturating_add(ttl_secs),
    };
    let approval_path = write_approval(store, &approval)
        .map_err(|e| anyhow::anyhow!("failed to write approval: {e}"))?;

    #[cfg(unix)]
    if rustix::process::geteuid().as_raw() == PRODUCTION_APPROVAL_UID {
        use std::os::unix::fs::PermissionsExt;
        let target_gid = find_group_gid("vaughan-signer").unwrap_or(store_gid);
        std::os::unix::fs::chown(
            &approval_path,
            Some(PRODUCTION_APPROVAL_UID),
            Some(target_gid),
        )
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to chown {} to root:vaughan-signer: {e}",
                approval_path.display()
            )
        })?;
        fs::set_permissions(&approval_path, fs::Permissions::from_mode(0o440)).map_err(|e| {
            anyhow::anyhow!(
                "failed to enforce mode 0440 on {}: {e}",
                approval_path.display()
            )
        })?;
    }

    #[cfg(not(unix))]
    let _ = store_gid;

    Ok(approval_path)
}
