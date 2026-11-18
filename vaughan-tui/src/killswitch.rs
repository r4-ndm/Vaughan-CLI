//! Ctrl+K kill switch: halt the in-process breaker **and** the signer daemon.
//!
//! Single purpose: decide whether Ctrl+K applies to this session, trip the local
//! session breaker, ask `vaughan signerd` to trip durably on a background thread,
//! and turn whatever the daemon answered into one honest flash line. The daemon is
//! the only thing that signs for a sentient profile, so a flash that says "halted"
//! when only the TUI-side breaker moved would be a lie the operator acts on.
//!
//! Must NOT contain: key dispatch, any other hotkey's behaviour, view/render code,
//! DCA scheduling, or signer IPC framing (that is `vaughan_core::core::SignerClient`).

use std::sync::mpsc::{self, Receiver, TryRecvError};

use tokio::runtime::Handle;
use vaughan_agent::CircuitBreaker;
use vaughan_core::core::signer_client::{is_too_old_for_trip, TRIP_UNSUPPORTED_ERR};
use vaughan_core::core::{SignerClient, SignerResponse};

use crate::blocking::block_on_handle;

/// Reason recorded in both breakers, so the daemon's audit line and trip file say
/// who stopped trading.
const KILL_REASON: &str = "manual kill switch (Ctrl+K)";

/// Longest daemon message we paste into a one-line flash.
const MAX_FLASH_DETAIL: usize = 120;

/// What the signer daemon did with our trip request.
///
/// Four different safety stories, which is why this is not a `bool`: only
/// [`Self::Halted`] means the thing that holds the keys is both stopped and still
/// stopped after a restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonTrip {
    /// Daemon halted and the trip was persisted (survives a restart).
    Halted,
    /// Daemon halted in memory but could not write its trip marker: a restart
    /// comes back signing.
    HaltedNotDurable(String),
    /// Daemon not reachable (socket missing, timeout, EOF): only the local breaker moved.
    NotReached(String),
    /// Daemon predates the `trip` op (or the `reason` field): it is NOT halted.
    TooOld,
    /// Daemon answered, reachable and new enough, but refused: NOT halted.
    Refused(String),
}

/// True when Ctrl+K has anything to stop.
///
/// Keeps today's gate: auto-exec only ever runs on an unlocked sentient profile, and
/// the daemon socket is machine-global — a human profile must not be able to halt a
/// daemon it never asked to sign for.
pub fn applies(is_unlocked: bool, profile_name: &str) -> bool {
    is_unlocked && crate::sentient_mcp::mcp_auto_exec_enabled(profile_name)
}

/// Map the daemon's answer to [`SignerClient::trip`] onto a [`DaemonTrip`].
pub fn classify(result: Result<SignerResponse, String>) -> DaemonTrip {
    match result {
        Ok(resp) if resp.ok => DaemonTrip::Halted,
        Ok(resp) => {
            let detail = resp
                .message
                .or_else(|| resp.code.clone())
                .unwrap_or_else(|| "no message".to_string());
            // The daemon tripped in memory but its marker is not on disk; it told us so
            // instead of claiming success.
            if resp.code.as_deref() == Some("trip_not_persisted") {
                DaemonTrip::HaltedNotDurable(detail)
            } else if is_too_old_for_trip(resp.code.as_deref()) {
                DaemonTrip::TooOld
            } else {
                DaemonTrip::Refused(detail)
            }
        }
        Err(e) if e == TRIP_UNSUPPORTED_ERR => DaemonTrip::TooOld,
        Err(e) => DaemonTrip::NotReached(e),
    }
}

/// The flash the operator reads. Every variant states whether the DAEMON stopped.
pub fn flash_line(outcome: &DaemonTrip) -> String {
    match outcome {
        DaemonTrip::Halted => "Sentient breaker TRIPPED — daemon HALTED (survives restart)".into(),
        DaemonTrip::HaltedNotDurable(e) => format!(
            "Sentient breaker TRIPPED — daemon HALTED but NOT on disk: {} — a restart comes back live",
            clip(e)
        ),
        DaemonTrip::NotReached(e) => format!(
            "Sentient breaker TRIPPED — daemon NOT reached: {} — local breaker only",
            clip(e)
        ),
        DaemonTrip::TooOld => {
            "Sentient breaker TRIPPED — daemon too old — NOT halted, stop the unit".into()
        }
        DaemonTrip::Refused(e) => format!(
            "Sentient breaker TRIPPED — daemon REFUSED: {} — NOT halted",
            clip(e)
        ),
    }
}

/// Truncate on a char boundary so a long daemon message cannot push the verdict
/// ("NOT halted") off the flash strip.
fn clip(detail: &str) -> String {
    let one_line: String = detail
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    match one_line.char_indices().nth(MAX_FLASH_DETAIL) {
        Some((cut, _)) => format!("{}…", &one_line[..cut]),
        None => one_line,
    }
}

/// In-flight daemon trip for the current session.
///
/// Ctrl+K must not block the event loop on a socket round trip, so the daemon's
/// answer arrives through this channel and [`Self::poll`] flashes it on a later tick.
#[derive(Default)]
pub struct KillSwitch {
    pending: Option<Receiver<DaemonTrip>>,
}

impl KillSwitch {
    /// Trip the local breaker now, start the daemon trip, and return the flash to show
    /// immediately. The honest verdict follows from [`Self::poll`].
    pub fn engage(&mut self, breaker: &CircuitBreaker, handle: &Handle) -> String {
        self.engage_with(breaker, handle, SignerClient::default())
    }

    /// [`Self::engage`] with the client injected: a test must point at its own socket
    /// path, never at the host daemon that signs real money.
    pub fn engage_with(
        &mut self,
        breaker: &CircuitBreaker,
        handle: &Handle,
        client: SignerClient,
    ) -> String {
        breaker.trip(KILL_REASON);
        tracing::warn!(
            target: "vaughan_tui::mcp",
            "sentient circuit breaker tripped manually; asking signer daemon to halt"
        );

        let (tx, rx) = mpsc::channel();
        let handle = handle.clone();
        // Background OS thread + the shared runtime handle: works on every runtime
        // flavour and adds no `block_in_place` to the UI thread.
        std::thread::spawn(move || {
            let sent = block_on_handle(&handle, async move { client.trip(KILL_REASON).await });
            let outcome = match sent {
                Ok(result) => classify(result),
                Err(e) => DaemonTrip::NotReached(e.to_string()),
            };
            let _ = tx.send(outcome);
        });
        self.pending = Some(rx);

        "Sentient breaker TRIPPED — asking the signer daemon to halt…".to_string()
    }

    /// Flash the daemon's verdict once it lands. `None` while nothing is in flight.
    pub fn poll(&mut self) -> Option<String> {
        let rx = self.pending.as_ref()?;
        match rx.try_recv() {
            Ok(outcome) => {
                self.pending = None;
                Some(flash_line(&outcome))
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                Some(flash_line(&DaemonTrip::NotReached(
                    "trip thread died before answering".to_string(),
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(code: &str, message: &str) -> SignerResponse {
        SignerResponse {
            ok: false,
            code: Some(code.to_string()),
            message: Some(message.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn applies_only_to_an_unlocked_sentient_profile() {
        assert!(applies(true, "sentient"));
        assert!(!applies(false, "sentient"));
        assert!(!applies(true, "default"));
    }

    #[test]
    fn classify_maps_each_daemon_answer() {
        let ok = SignerResponse {
            ok: true,
            message: Some("daemon halted: kill switch: x".into()),
            ..Default::default()
        };
        assert_eq!(classify(Ok(ok)), DaemonTrip::Halted);
        assert_eq!(
            classify(Ok(refused("trip_not_persisted", "disk full"))),
            DaemonTrip::HaltedNotDurable("disk full".into())
        );
        // Both ways an old daemon refuses the op mean the same thing: NOT halted.
        assert_eq!(
            classify(Ok(refused("unknown_op", "unrecognized op 'trip'"))),
            DaemonTrip::TooOld
        );
        assert_eq!(
            classify(Ok(refused("invalid_json", "unknown field `reason`"))),
            DaemonTrip::TooOld
        );
        assert_eq!(
            classify(Err(TRIP_UNSUPPORTED_ERR.to_string())),
            DaemonTrip::TooOld
        );
        assert_eq!(
            classify(Ok(refused("internal_error", "boom"))),
            DaemonTrip::Refused("boom".into())
        );
        assert_eq!(
            classify(Err("signer_unavailable: cannot connect".into())),
            DaemonTrip::NotReached("signer_unavailable: cannot connect".into())
        );
    }

    #[test]
    fn flash_lines_say_whether_the_daemon_stopped() {
        assert!(flash_line(&DaemonTrip::Halted).contains("daemon HALTED"));

        let not_reached = flash_line(&DaemonTrip::NotReached("socket missing".into()));
        assert!(not_reached.contains("daemon NOT reached: socket missing"));
        assert!(not_reached.contains("local breaker only"));

        let too_old = flash_line(&DaemonTrip::TooOld);
        assert!(too_old.contains("daemon too old"));
        assert!(too_old.contains("NOT halted"));

        assert!(
            flash_line(&DaemonTrip::HaltedNotDurable("disk full".into())).contains("NOT on disk")
        );
        assert!(flash_line(&DaemonTrip::Refused("nope".into())).contains("NOT halted"));
    }

    #[test]
    fn clip_strips_control_chars_and_keeps_the_verdict_visible() {
        // An attacker-ish daemon message must not clear the screen or hide the verdict.
        let line = flash_line(&DaemonTrip::NotReached("\u{1b}[2J\nboom".into()));
        assert!(!line.contains('\n'));
        assert!(!line.contains('\u{1b}'));
        assert!(line.contains("local breaker only"));

        // Multi-byte chars straddling the cut: a byte-wise truncation panics here.
        let long = flash_line(&DaemonTrip::NotReached(format!("x{}", "€".repeat(500))));
        assert!(long.contains('…'));
        assert!(long.contains("local breaker only"));
    }

    #[test]
    fn engage_trips_the_local_breaker_and_poll_reports_the_daemon() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let breaker = CircuitBreaker::new(Default::default());
        let mut ks = KillSwitch::default();

        let nowhere = std::env::temp_dir().join("vaughan-killswitch-test-absent.sock");
        let first = ks.engage_with(&breaker, rt.handle(), SignerClient::new(&nowhere));
        assert!(breaker.is_tripped());
        assert!(breaker.trip_reason().unwrap().contains("Ctrl+K"));
        assert!(first.contains("TRIPPED"));

        // Nothing is listening on that path: the honest verdict is "not reached",
        // never "halted".
        let verdict = loop {
            if let Some(v) = ks.poll() {
                break v;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(!verdict.contains("daemon HALTED"), "{verdict}");
        assert!(ks.poll().is_none(), "verdict must be flashed once");
    }
}
