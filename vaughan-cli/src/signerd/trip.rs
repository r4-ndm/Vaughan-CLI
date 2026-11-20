//! The daemon's durable kill switch: the `trip` op, `signerd-tripped.json`
//! persistence, and the fail-closed load that runs before the socket is bound.
//!
//! Single purpose: turning "this daemon is halted" into a fact that survives a
//! crash, an OOM kill, or `Restart=on-failure` — the in-memory
//! [`vaughan_agent::CircuitBreaker`] flag alone does not. Must NOT contain
//! policy logic, intent handling, or any way to UNTRIP: clearing a trip is
//! root deleting the file and restarting the unit (see docs/agents/sentient-ops.md).

use std::path::Path;

use serde::{Deserialize, Serialize};
use vaughan_core::core::signer_client::SignerResponse;

use super::Signer;

/// Marker file inside the signer store recording that trading is halted.
pub const TRIP_FILE: &str = "signerd-tripped.json";

/// Longest reason kept, in chars and in bytes (see [`sanitise_reason`]).
const REASON_MAX: usize = 200;

/// On-disk kill-switch record. Written once per trip and never updated: the first
/// reason wins on disk exactly as it does in the breaker.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TripRecord {
    /// Already-sanitised reason reported back to every later `sign` request.
    reason: String,
    /// When the trip was persisted.
    tripped_at_unix: u64,
    /// `SO_PEERCRED` UID of the peer that asked, or `None` for an automatic trip
    /// (gas ceiling / consecutive-error tripwire), where no peer asked.
    peer_uid: Option<u32>,
}

/// What `<store>/signerd-tripped.json` says at startup.
pub(super) enum TripOnDisk {
    /// No marker file: the daemon starts untripped (normal operation).
    Absent,
    /// A marker file exists, or exists but cannot be trusted. Either way the daemon
    /// starts tripped — an unreadable kill switch must never read as "not halted".
    Tripped(String),
}

/// Strip control characters, then clamp to [`REASON_MAX`] chars AND bytes on a char
/// boundary, BEFORE the reason is ever formatted into a message, a log line, or a file:
/// the reason is attacker-controlled free text from the socket and ends up in an
/// operator's terminal (`"\u{1b}[2J"` would clear their screen) and in the audit log.
pub(super) fn sanitise_reason(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(REASON_MAX)
        .collect();
    if s.len() > REASON_MAX {
        let mut end = REASON_MAX;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// Read the kill-switch marker, failing CLOSED: a file that exists but has the wrong
/// mode/owner, cannot be read, or does not parse means the daemon starts TRIPPED. Only a
/// genuinely absent file means "not halted", so a corrupted marker cannot silently clear
/// a trip (an attacker able to truncate the file must not be able to resume trading).
pub(super) fn load_trip(store: &Path) -> TripOnDisk {
    let path = store.join(TRIP_FILE);
    match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return TripOnDisk::Absent,
        Err(e) => {
            return TripOnDisk::Tripped(format!(
                "kill switch: {} is unreadable ({e}) — failing closed; \
                 root must inspect and remove it, then restart the unit",
                path.display()
            ));
        }
        Ok(_) => {}
    }

    if let Err(e) = vaughan_agent::sentient::assert_owner_only_file(&path) {
        return TripOnDisk::Tripped(format!(
            "kill switch: {} is not trusted ({e}) — failing closed",
            path.display()
        ));
    }

    let unusable = |what: &str| {
        TripOnDisk::Tripped(format!(
            "kill switch: {} could not be read as a trip record ({what}) — failing closed; \
             root must inspect and remove it, then restart the unit",
            path.display()
        ))
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return unusable("read failed");
    };
    match serde_json::from_str::<TripRecord>(&raw) {
        Ok(rec) => TripOnDisk::Tripped(sanitise_reason(&rec.reason)),
        Err(e) => unusable(&e.to_string()),
    }
}

/// Write the marker atomically (temp file at mode 0600 -> fsync -> rename), and only if it
/// is absent: the first trip's reason is the one that halted trading and must not be
/// overwritten by a later trip or by the automatic-trip persistence pass.
fn persist_trip_if_absent(store: &Path, reason: &str, peer_uid: Option<u32>) -> Result<(), String> {
    let path = store.join(TRIP_FILE);
    if path.exists() {
        return Ok(());
    }
    let record = TripRecord {
        reason: reason.to_string(),
        tripped_at_unix: vaughan_agent::dca::now_unix(),
        peer_uid,
    };
    let body = serde_json::to_string(&record).map_err(|e| format!("serialize trip record: {e}"))?;

    let tmp = store.join(format!(".{TRIP_FILE}.tmp"));
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .map_err(|e| format!("failed to open {}: {e}", tmp.display()))?;
        f.write_all(body.as_bytes())
            .map_err(|e| format!("failed to write {}: {e}", tmp.display()))?;
        // fsync before rename: a trip that only reached the page cache would be lost by
        // exactly the crash this file exists to survive.
        f.sync_all()
            .map_err(|e| format!("failed to fsync {}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("failed to replace {}: {e}", path.display())
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

impl Signer {
    /// Handle `{"op":"trip","reason":"…"}`: halt this daemon now and keep it halted.
    ///
    /// Any peer the `SO_PEERCRED` allowlist already admits may trip — stopping is the safe
    /// direction, so it needs no root step-up. There is deliberately no untrip op.
    pub fn handle_trip(&self, reason: Option<String>, peer_uid: u32) -> SignerResponse {
        let requested = sanitise_reason(reason.as_deref().unwrap_or("no reason given"));
        let message = format!("kill switch: {requested}");

        // Trip first: a failure to write the marker or the audit line must not leave the
        // daemon signing.
        self.inner.breaker.lock().unwrap().trip(message.clone());
        // First-reason-wins: report and persist whatever actually halted trading, which may
        // be an earlier automatic trip rather than this request.
        let effective = self
            .inner
            .breaker
            .lock()
            .unwrap()
            .trip_state()
            .1
            .unwrap_or(message);

        let mut notes = Vec::new();
        if let Err(e) = vaughan_agent::sentient::append_trip_audit(
            &self.inner.store,
            Some(peer_uid),
            &requested,
        ) {
            tracing::error!("trip audit line not written: {e}");
            notes.push(format!("audit line NOT written: {e}"));
        }

        if let Err(e) = persist_trip_if_absent(&self.inner.store, &effective, Some(peer_uid)) {
            tracing::error!("trip not persisted: {e}");
            return SignerResponse::error(
                "trip_not_persisted",
                format!(
                    "daemon is halted in memory, but {TRIP_FILE} could not be written ({e}): a \
                     restart would come back UNTRIPPED — stop the unit"
                ),
            );
        }

        let mut msg = format!("daemon halted: {effective}");
        for note in notes {
            msg.push_str(" (");
            msg.push_str(&note);
            msg.push(')');
        }
        SignerResponse {
            ok: true,
            message: Some(msg),
            ..Default::default()
        }
    }

    /// Persist an AUTOMATIC trip (gas ceiling, consecutive-error tripwire) that the breaker
    /// raised while handling a request. Called after every `sign`: those trips never pass
    /// through [`Self::handle_trip`], and an unpersisted one evaporates on the next restart.
    pub(super) fn persist_automatic_trip(&self) {
        let (tripped, reason) = self.inner.breaker.lock().unwrap().trip_state();
        if !tripped {
            return;
        }
        let reason = sanitise_reason(&reason.unwrap_or_else(|| "halted".into()));
        if self.inner.store.join(TRIP_FILE).exists() {
            return;
        }
        if let Err(e) = vaughan_agent::sentient::append_trip_audit(&self.inner.store, None, &reason)
        {
            tracing::error!("automatic trip audit line not written: {e}");
        }
        if let Err(e) = persist_trip_if_absent(&self.inner.store, &reason, None) {
            tracing::error!(
                "automatic trip not persisted to {TRIP_FILE}: {e} — a restart would come back \
                 UNTRIPPED"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitise_reason_strips_control_chars_and_clamps() {
        assert_eq!(sanitise_reason("\u{1b}[2J\nx"), "[2Jx");
        assert_eq!(sanitise_reason("ok"), "ok");

        let long = "a".repeat(500);
        assert_eq!(sanitise_reason(&long).len(), REASON_MAX);

        // Multi-byte: 200 chars of 'é' is 400 bytes, so the byte clamp must land on a
        // char boundary and keep valid UTF-8.
        let wide = "é".repeat(500);
        let out = sanitise_reason(&wide);
        assert!(out.len() <= REASON_MAX);
        assert!(out.chars().all(|c| c == 'é'));
    }

    #[test]
    fn load_trip_absent_then_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(load_trip(dir.path()), TripOnDisk::Absent));

        persist_trip_if_absent(dir.path(), "kill switch: stop", Some(1000)).unwrap();
        match load_trip(dir.path()) {
            TripOnDisk::Tripped(r) => assert_eq!(r, "kill switch: stop"),
            TripOnDisk::Absent => panic!("a persisted trip must load as tripped"),
        }

        // First reason wins on disk.
        persist_trip_if_absent(dir.path(), "kill switch: later", Some(1001)).unwrap();
        match load_trip(dir.path()) {
            TripOnDisk::Tripped(r) => assert_eq!(r, "kill switch: stop"),
            TripOnDisk::Absent => unreachable!(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn persisted_trip_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        persist_trip_if_absent(dir.path(), "kill switch: mode", Some(0)).unwrap();
        let mode = std::fs::metadata(dir.path().join(TRIP_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn corrupt_trip_file_fails_closed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TRIP_FILE);
        std::fs::write(&path, b"{ not json").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        match load_trip(dir.path()) {
            TripOnDisk::Tripped(r) => assert!(r.contains("failing closed"), "{r}"),
            TripOnDisk::Absent => panic!("a corrupt trip file must start the daemon TRIPPED"),
        }

        // Wrong mode is equally untrusted.
        std::fs::write(
            &path,
            br#"{"reason":"x","tripped_at_unix":1,"peer_uid":null}"#,
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        match load_trip(dir.path()) {
            TripOnDisk::Tripped(r) => assert!(r.contains("not trusted"), "{r}"),
            TripOnDisk::Absent => panic!("an untrusted trip file must start the daemon TRIPPED"),
        }
    }
}
