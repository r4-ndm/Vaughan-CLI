//! Local phishing / malicious-address deny-list.
//!
//! Vaughan has no hosted blocklist and makes no network calls for this (the
//! no-telemetry rule). Instead a small, curated set of known-bad addresses is
//! bundled with the binary, and the user can add their own entries which are
//! persisted with the rest of the profile state.
//!
//! The list is checked by the humanizer and at provider-approval time: any
//! transaction or typed-data whose `to` / `spender` / `verifyingContract` is
//! denied gets a hard warning and an extra explicit confirm.
//!
//! **Keep the bundled list small and high-confidence.** False positives block
//! legitimate contracts, so only add addresses that are unambiguous drainer /
//! scam infrastructure. Users can always override with their own list.

use alloy::primitives::Address;
use serde::{Deserialize, Serialize};

/// A user-supplied deny-list entry, persisted in the profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DenyEntry {
    /// Lowercase hex address (with or without `0x`).
    pub address: String,
    /// Why it was added (shown on the warning).
    #[serde(default)]
    pub reason: String,
}

/// Bundled deny-list: `(address, reason)`, chain-agnostic, matched
/// case-insensitively.
///
/// **Intentionally empty for now.** An entry here blocks a contract for every
/// user, so the bar is: the address is verified drainer / phishing
/// infrastructure from a primary source (on-chain evidence or the protocol's
/// own disclosure), recorded with that source in the reason string. Do not add
/// addresses from memory or unverified lists. Until entries exist, protection
/// comes from the user's own entries plus the humanizer's structural warnings
/// (unlimited approvals, unknown spenders, `setApprovalForAll`, …).
const BUNDLED: &[(&str, &str)] = &[];

/// True when `address` is on the bundled list or the user's own entries.
///
/// `user_entries` comes from the persisted profile (`PersistedState::denylist`).
pub fn is_denied(address: Address, user_entries: &[DenyEntry]) -> bool {
    is_bundled(address) || find_user(address, user_entries).is_some()
}

/// True when `address` is on the bundled list only.
pub fn is_bundled(address: Address) -> bool {
    find_bundled(address).is_some()
}

/// Reason a denied address was listed: the user's reason, else the bundled
/// reason, else a generic fallback.
pub fn deny_reason(address: Address, user_entries: &[DenyEntry]) -> String {
    if let Some(e) = find_user(address, user_entries) {
        if !e.reason.is_empty() {
            return e.reason.clone();
        }
    }
    if let Some((_, reason)) = find_bundled(address) {
        return (*reason).to_string();
    }
    "on the phishing deny-list".to_string()
}

fn find_user(address: Address, user_entries: &[DenyEntry]) -> Option<&DenyEntry> {
    let norm = normalize(&format!("{address:#x}"));
    user_entries.iter().find(|e| normalize(&e.address) == norm)
}

fn find_bundled(address: Address) -> Option<&'static (&'static str, &'static str)> {
    let norm = normalize(&format!("{address:#x}"));
    BUNDLED.iter().find(|(a, _)| normalize(a) == norm)
}

/// Lowercase, strip `0x`, trim — so `0xABC`, `abc`, ` ABC ` all match.
fn normalize(s: &str) -> String {
    s.trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn bundled_entries_parse_as_addresses() {
        // Guards against a typo in a future bundled entry silently never matching.
        for (addr, reason) in BUNDLED {
            assert!(
                addr.parse::<Address>().is_ok(),
                "bad bundled address {addr}"
            );
            assert!(
                !reason.is_empty(),
                "bundled entry {addr} needs a sourced reason"
            );
        }
    }

    #[test]
    fn empty_user_list_denies_nothing_extra() {
        assert_eq!(is_denied(Address::ZERO, &[]), is_bundled(Address::ZERO));
    }

    #[test]
    fn user_entry_matches_case_insensitively() {
        let entries = vec![DenyEntry {
            address: "0x00000000000000000000000000000000DEADBEEF".into(),
            reason: "scam".into(),
        }];
        let addr = address!("0x00000000000000000000000000000000deadbeef");
        assert!(is_denied(addr, &entries));
        assert_eq!(deny_reason(addr, &entries), "scam");
    }

    #[test]
    fn clean_address_is_not_denied() {
        let addr = address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9");
        assert!(!is_denied(addr, &[]));
    }

    #[test]
    fn normalize_handles_prefix_and_case() {
        assert_eq!(normalize("0xABC "), "abc");
        assert_eq!(normalize("0XDEF"), "def");
    }
}
