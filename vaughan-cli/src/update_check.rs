//! The opt-in "a newer version exists" check. **Off by default.**
//!
//! Why off: a self-custody wallet that contacts GitHub on every launch tells
//! that host, on a schedule, that this IP runs Vaughan and which version — a
//! standing correlation handle next to stealth addresses and RAILGUN. Coding
//! tools check by default because their launch is not sensitive; a wallet's is.
//! So the beacon exists only when the user asks for it:
//! `vaughan config update-check on`.
//!
//! State lives in its own small file, NOT in `wallet.json`. Two reasons: that
//! file carries the encrypted vault and nothing as trivial as "when did we last
//! look for an update" justifies rewriting it, and the answer is about the
//! installed binary, which is machine-wide rather than per-profile.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Minimum gap between two network checks.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a launch may wait for the check before giving up on it.
///
/// Short on purpose: this runs before the unlock prompt, so the cost of a slow
/// or unreachable host has to be bounded by something the user would not notice,
/// not by the HTTP client's own timeout.
pub const LAUNCH_CHECK_DEADLINE: Duration = Duration::from_secs(3);

/// The opt-in flag plus last-check bookkeeping.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    /// Whether the periodic check may run at all. Absent means false.
    #[serde(default)]
    enabled: bool,
    /// Unix seconds of the last attempted check.
    #[serde(default)]
    last_checked: u64,
    /// The newest tag seen, so a notice survives until the user updates.
    #[serde(default)]
    latest_seen: String,
}

/// Overrides [`state_path`]. Exists so tests can exercise the real load/save
/// path without writing to the developer's own data directory.
const STATE_PATH_ENV: &str = "VAUGHAN_UPDATE_STATE";

fn state_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os(STATE_PATH_ENV) {
        return Ok(PathBuf::from(p));
    }
    let base = dirs::data_dir().context("could not determine the user data directory")?;
    Ok(base.join("vaughan-cli").join("update-check.json"))
}

fn load() -> State {
    state_path()
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(state: &State) -> Result<()> {
    let path = state_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("could not create the data directory")?;
    }
    let body = serde_json::to_vec_pretty(state).context("could not serialize update state")?;
    // Write-then-rename: an interrupted write must not leave invalid JSON that
    // silently resets the opt-in to off.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &body).context("could not write the update state")?;
    std::fs::rename(&tmp, &path).context("could not replace the update state")?;
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether the user opted in. False when the file is missing or unreadable.
pub fn is_enabled() -> bool {
    load().enabled
}

/// Turn the periodic check on or off, preserving the cache.
pub fn set_enabled(enabled: bool) -> Result<()> {
    let mut state = load();
    state.enabled = enabled;
    save(&state)
}

/// A newer version to tell the user about, or `None`.
///
/// Returns `None` without touching the network whenever the opt-in is off,
/// which is the default. Network and parse failures are also `None`: a failed
/// update check must never be something the user has to deal with on launch.
pub async fn notice() -> Option<String> {
    let mut state = load();
    if !state.enabled {
        return None;
    }
    let current = env!("CARGO_PKG_VERSION");

    if now_secs().saturating_sub(state.last_checked) >= CHECK_INTERVAL.as_secs() {
        // Persist the attempt BEFORE awaiting, not after. The launch path wraps
        // this in `LAUNCH_CHECK_DEADLINE`, so the await below can be cancelled
        // and never resume — and a save placed after it would then never run,
        // turning an unreachable host into a fresh timeout on every launch.
        state.last_checked = now_secs();
        let _ = save(&state);
        if let Ok(tag) = crate::update::latest_tag().await {
            state.latest_seen = tag;
            let _ = save(&state);
        }
    }

    if !state.latest_seen.is_empty() && crate::update::is_newer(&state.latest_seen, current) {
        Some(state.latest_seen)
    } else {
        None
    }
}

/// The opening TUI toast when a newer release is known, else `None`.
///
/// Lives here rather than in `main` because it is the launch-path wrapper around
/// [`notice`]: it checks the opt-in first and returns before building a runtime
/// when the check is off (the default), then bounds the whole thing by
/// [`LAUNCH_CHECK_DEADLINE`] — this sits between the user and their wallet, so a
/// slow or blackholed host must cost a bounded pause, never the launch.
pub fn startup_notice() -> Option<String> {
    if !is_enabled() {
        return None;
    }
    let runtime = tokio::runtime::Runtime::new().ok()?;
    let latest = runtime.block_on(async {
        tokio::time::timeout(LAUNCH_CHECK_DEADLINE, notice())
            .await
            .ok()
            .flatten()
    })?;
    Some(format!(
        "Update available: {latest} (you have {}) — run `vaughan update`",
        env!("CARGO_PKG_VERSION")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default-off guarantee: with no state file (or `enabled: false`), the
    /// check returns `None` via the early return, before any network use.
    #[test]
    fn the_check_is_skipped_entirely_when_disabled() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        if is_enabled() {
            // A developer machine with the opt-in on would otherwise make this
            // test hit the network; the guarantee under test is the off path.
            return;
        }
        assert_eq!(rt.block_on(notice()), None);
    }

    #[test]
    fn absent_state_reads_as_off() {
        let state: State = serde_json::from_str("{}").expect("empty object parses");
        assert!(!state.enabled);
        assert_eq!(state.last_checked, 0);
        assert!(state.latest_seen.is_empty());
    }

    /// A cached newer tag is reported without a network call, which is both the
    /// 24h-cache behaviour and the only way to test the notice offline.
    ///
    /// This is the path that was silently dead before: the notice used to be
    /// set as a chrome flash, which the unlock screen does not render, so it
    /// expired unseen. It is now routed via `App::surface_message`.
    #[test]
    fn a_cached_newer_tag_is_reported_from_the_state_file() {
        let dir = std::env::temp_dir().join(format!("vaughan-uc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("update-check.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "enabled": true,
                // Fresh, so the 24h gate short-circuits before any request.
                "last_checked": now_secs(),
                "latest_seen": "v99.0.0",
            }))
            .expect("json"),
        )
        .expect("seed state");

        // The override is process-global, so it is set and removed tightly
        // around the call. The sibling test tolerates seeing it: it bails out
        // early whenever the opt-in reads as on.
        std::env::set_var(STATE_PATH_ENV, &path);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let got = rt.block_on(notice());
        let enabled = is_enabled();
        std::env::remove_var(STATE_PATH_ENV);
        let _ = std::fs::remove_dir_all(&dir);

        assert!(enabled, "the seeded state must read as opted in");
        assert_eq!(got.as_deref(), Some("v99.0.0"));
    }
}
