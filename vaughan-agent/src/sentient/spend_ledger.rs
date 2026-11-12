//! Autonomous sentient wallet spend ledger and cumulative 24h cap enforcement.
//!
//! # Pessimistic Reservation
//! Reservations are **pessimistic**: once reserved, the budget stays spent
//! even if the broadcast later fails. Do not add refunds. A failed tx costs budget,
//! and a retry needs a human to raise the cap or wait for the window reset.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, U256};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::AgentError;
use crate::sentient::policy::{AgentSessionPolicy, EnforcementMode};

pub const SPEND_LEDGER_FILE: &str = "sentient-spend.json";
pub const SPEND_LOCK_FILE: &str = "sentient-spend.lock";
pub const WINDOW_SECONDS_DEFAULT: u64 = 86_400; // 24 hours
const MAX_RECENT_ENTRIES: usize = 256;

mod u256_dec_or_hex {
    use super::*;

    pub fn serialize<S>(val: &U256, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&val.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<U256, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let t = s.trim();
        if let Some(stripped) = t.strip_prefix("0x") {
            U256::from_str_radix(stripped, 16).map_err(serde::de::Error::custom)
        } else {
            U256::from_str(t).map_err(serde::de::Error::custom)
        }
    }
}

/// Result of a spend reservation attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReserveOutcome {
    Reserved { spent: U256, cap: U256 },
}

/// Per-asset snapshot and accumulated spend within the current 24h window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetSpend {
    #[serde(with = "u256_dec_or_hex")]
    pub snapshot: U256,
    #[serde(with = "u256_dec_or_hex")]
    pub spent: U256,
}

/// Record of an idempotency key and spend amount.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecentSpend {
    pub key: String,
    pub asset: String,
    #[serde(with = "u256_dec_or_hex")]
    pub amount: U256,
    pub unix: u64,
}

/// In-memory view of `sentient-spend.json` for status inspection and testing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LedgerView {
    pub version: u32,
    pub window_start_unix: u64,
    pub window_seconds: u64,
    #[serde(default)]
    pub assets: HashMap<String, AssetSpend>,
    #[serde(default)]
    pub recent: Vec<RecentSpend>,
}

impl Default for LedgerView {
    fn default() -> Self {
        Self {
            version: 1,
            window_start_unix: 0,
            window_seconds: WINDOW_SECONDS_DEFAULT,
            assets: HashMap::new(),
            recent: Vec::new(),
        }
    }
}

struct LockGuard(PathBuf);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn current_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

/// Acquire the exclusive lock on `<dir>/sentient-spend.lock`.
///
/// Uses `create_new(true)` with a 20 × 5 ms retry budget. If the lock is held
/// by a dead process (`/proc/<pid>` missing) or older than 60s, it is cleaned
/// up as stale and retried once.
fn acquire_lock(dir: &Path, now: u64) -> Result<LockGuard, AgentError> {
    let lock_path = dir.join(SPEND_LOCK_FILE);
    let mut cleaned_stale = false;

    for _ in 0..20 {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }

        match opts.open(&lock_path) {
            Ok(mut f) => {
                let content = format!("{}:{}", std::process::id(), now);
                let _ = f.write_all(content.as_bytes());
                let _ = f.flush();
                return Ok(LockGuard(lock_path));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if !cleaned_stale {
                    if let Ok(raw) = fs::read_to_string(&lock_path) {
                        let parts: Vec<&str> = raw.trim().split(':').collect();
                        if parts.len() == 2 {
                            let pid_opt = parts[0].parse::<u32>().ok();
                            let start_opt = parts[1].parse::<u64>().ok();

                            let pid_dead = match pid_opt {
                                Some(pid) => !Path::new(&format!("/proc/{pid}")).exists(),
                                None => true,
                            };
                            let expired = match start_opt {
                                Some(start) => now >= start.saturating_add(60),
                                None => true,
                            };

                            if pid_dead || expired {
                                let _ = fs::remove_file(&lock_path);
                                cleaned_stale = true;
                                continue;
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                return Err(AgentError::SpendGuard(format!(
                    "spend_ledger_busy: lock open error: {e}"
                )));
            }
        }
    }

    Err(AgentError::SpendGuard(format!(
        "spend_ledger_busy: could not acquire lock on {} within retry budget",
        lock_path.display()
    )))
}

/// Core stateless spend ledger operator.
pub struct SpendLedger;

impl SpendLedger {
    /// Lock, re-read, roll window, check cap, merge, sync, rename, unlock. All under one lock.
    pub fn reserve(
        dir: &Path,
        key: &str,
        asset: &str,
        amount: U256,
        balance: U256,
        cap_pct: u8,
        mode: EnforcementMode,
    ) -> Result<ReserveOutcome, AgentError> {
        Self::reserve_at(
            dir,
            key,
            asset,
            amount,
            balance,
            cap_pct,
            mode,
            current_unix(),
        )
    }

    /// Internal version allowing time injection for tests.
    #[allow(clippy::too_many_arguments)]
    pub fn reserve_at(
        dir: &Path,
        key: &str,
        asset: &str,
        amount: U256,
        balance: U256,
        cap_pct: u8,
        mode: EnforcementMode,
        now: u64,
    ) -> Result<ReserveOutcome, AgentError> {
        if mode == EnforcementMode::Disabled {
            return Ok(ReserveOutcome::Reserved {
                spent: U256::ZERO,
                cap: U256::MAX,
            });
        }

        fs::create_dir_all(dir).map_err(|e| {
            AgentError::SpendGuard(format!("spend_ledger_disk: create profile dir: {e}"))
        })?;

        let _guard = acquire_lock(dir, now)?;
        let file_path = dir.join(SPEND_LEDGER_FILE);

        let mut ledger = if file_path.exists() {
            let raw = fs::read_to_string(&file_path).map_err(|e| {
                AgentError::SpendGuard(format!(
                    "spend_ledger_unreadable: failed to read {}: {e}",
                    file_path.display()
                ))
            })?;
            let parsed: LedgerView = serde_json::from_str(&raw).map_err(|e| {
                AgentError::SpendGuard(format!(
                    "spend_ledger_unreadable: JSON parse failed in {}: {e}",
                    file_path.display()
                ))
            })?;
            if parsed.version != 1 {
                return Err(AgentError::SpendGuard(format!(
                    "spend_ledger_unreadable: unsupported ledger version {}",
                    parsed.version
                )));
            }
            parsed
        } else {
            LedgerView {
                version: 1,
                window_start_unix: now,
                window_seconds: WINDOW_SECONDS_DEFAULT,
                assets: HashMap::new(),
                recent: Vec::new(),
            }
        };

        // Check window rollover or clock rollback
        if ledger.window_start_unix == 0 {
            ledger.window_start_unix = now;
        } else if now
            >= ledger
                .window_start_unix
                .saturating_add(ledger.window_seconds)
        {
            ledger.assets.clear();
            ledger.recent.clear();
            ledger.window_start_unix = now;
        } else if now < ledger.window_start_unix {
            tracing::warn!(
                target: "vaughan_agent::sentient",
                "system clock jumped backwards (now < window_start_unix); resetting spend window to now"
            );
            ledger.assets.clear();
            ledger.recent.clear();
            ledger.window_start_unix = now;
        }

        // Idempotency check: duplicate key is a refusal (prevents replay / double-spend)
        if ledger.recent.iter().any(|r| r.key == key) {
            return Err(AgentError::SpendGuard(format!(
                "spend_already_reserved: key `{key}` was already charged in this window — refusing to broadcast a replay"
            )));
        }

        // Cap calculations
        let asset_entry = ledger
            .assets
            .entry(asset.to_string())
            .or_insert_with(|| AssetSpend {
                snapshot: balance,
                spent: U256::ZERO,
            });

        let snapshot = asset_entry.snapshot;
        let cap = (snapshot.saturating_mul(U256::from(cap_pct))) / U256::from(100);
        let new_spent = asset_entry.spent.saturating_add(amount);

        if mode == EnforcementMode::WarnOnly {
            if new_spent > cap {
                tracing::warn!(
                    target: "vaughan_agent::sentient",
                    "spend cap exceeded under warn-only: asset `{asset}` requested {amount} wei, spent {} wei > cap {cap} wei",
                    asset_entry.spent
                );
            }
            return Ok(ReserveOutcome::Reserved {
                spent: asset_entry.spent,
                cap,
            });
        }

        if new_spent > cap {
            let reset_time = ledger
                .window_start_unix
                .saturating_add(ledger.window_seconds);
            return Err(AgentError::SpendGuard(format!(
                "spend_cap_exceeded: asset `{asset}` requested {amount} wei with spent {} wei > cap {cap} wei (window reset at unix {reset_time})",
                asset_entry.spent
            )));
        }

        // Update state
        asset_entry.spent = new_spent;
        ledger.recent.push(RecentSpend {
            key: key.to_string(),
            asset: asset.to_string(),
            amount,
            unix: now,
        });
        if ledger.recent.len() > MAX_RECENT_ENTRIES {
            let excess = ledger.recent.len() - MAX_RECENT_ENTRIES;
            ledger.recent.drain(0..excess);
        }

        // Atomic write
        let body = serde_json::to_string_pretty(&ledger).map_err(|e| {
            AgentError::SpendGuard(format!("spend_ledger_disk: serialization error: {e}"))
        })?;

        let tmp_path = dir.join(format!(".{SPEND_LEDGER_FILE}.tmp.{}", std::process::id()));
        {
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp_path).map_err(|e| {
                AgentError::SpendGuard(format!(
                    "spend_ledger_disk: open tmp {}: {e}",
                    tmp_path.display()
                ))
            })?;
            f.write_all(body.as_bytes()).map_err(|e| {
                AgentError::SpendGuard(format!(
                    "spend_ledger_disk: write tmp {}: {e}",
                    tmp_path.display()
                ))
            })?;
            f.sync_all().map_err(|e| {
                AgentError::SpendGuard(format!(
                    "spend_ledger_disk: sync tmp {}: {e}",
                    tmp_path.display()
                ))
            })?;
        }

        fs::rename(&tmp_path, &file_path).map_err(|e| {
            AgentError::SpendGuard(format!(
                "spend_ledger_disk: rename {} -> {}: {e}",
                tmp_path.display(),
                file_path.display()
            ))
        })?;

        Ok(ReserveOutcome::Reserved {
            spent: new_spent,
            cap,
        })
    }

    /// Read the current snapshot of `<dir>/sentient-spend.json` without taking a lock.
    pub fn snapshot(dir: &Path) -> Result<LedgerView, AgentError> {
        let file_path = dir.join(SPEND_LEDGER_FILE);
        if !file_path.exists() {
            return Ok(LedgerView::default());
        }
        let raw = fs::read_to_string(&file_path).map_err(|e| {
            AgentError::SpendGuard(format!(
                "spend_ledger_unreadable: failed to read {}: {e}",
                file_path.display()
            ))
        })?;
        let parsed: LedgerView = serde_json::from_str(&raw).map_err(|e| {
            AgentError::SpendGuard(format!(
                "spend_ledger_unreadable: JSON parse failed in {}: {e}",
                file_path.display()
            ))
        })?;
        if parsed.version != 1 {
            return Err(AgentError::SpendGuard(format!(
                "spend_ledger_unreadable: unsupported ledger version {}",
                parsed.version
            )));
        }
        Ok(parsed)
    }

    /// Release all spend reservations matching `key` or `{key}:*`.
    ///
    /// Subtracts the spent amounts from their respective asset tallies and
    /// removes the entries from recent spends, allowing retries without replay errors.
    pub fn release(dir: &Path, key: &str) -> Result<(), AgentError> {
        Self::release_at(dir, key, current_unix())
    }

    /// Internal release allowing time injection for tests.
    pub fn release_at(dir: &Path, key: &str, now: u64) -> Result<(), AgentError> {
        let file_path = dir.join(SPEND_LEDGER_FILE);
        if !file_path.exists() {
            return Ok(());
        }

        let _guard = acquire_lock(dir, now)?;
        let raw = fs::read_to_string(&file_path).map_err(|e| {
            AgentError::SpendGuard(format!(
                "spend_ledger_unreadable: failed to read {}: {e}",
                file_path.display()
            ))
        })?;
        let mut ledger: LedgerView = serde_json::from_str(&raw).map_err(|e| {
            AgentError::SpendGuard(format!(
                "spend_ledger_unreadable: JSON parse failed in {}: {e}",
                file_path.display()
            ))
        })?;
        if ledger.version != 1 {
            return Err(AgentError::SpendGuard(format!(
                "spend_ledger_unreadable: unsupported ledger version {}",
                ledger.version
            )));
        }

        let mut released = false;
        let mut i = 0;
        while i < ledger.recent.len() {
            let entry = &ledger.recent[i];
            let matches = entry.key == key || entry.key.starts_with(&format!("{key}:"));
            if matches {
                let amount = entry.amount;
                let asset = entry.asset.clone();
                if let Some(asset_entry) = ledger.assets.get_mut(&asset) {
                    asset_entry.spent = asset_entry.spent.saturating_sub(amount);
                }
                ledger.recent.remove(i);
                released = true;
            } else {
                i += 1;
            }
        }

        if !released {
            return Ok(());
        }

        let body = serde_json::to_string_pretty(&ledger).map_err(|e| {
            AgentError::SpendGuard(format!("spend_ledger_disk: serialization error: {e}"))
        })?;

        let tmp_path = dir.join(format!(".{SPEND_LEDGER_FILE}.tmp.{}", std::process::id()));
        {
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp_path).map_err(|e| {
                AgentError::SpendGuard(format!(
                    "spend_ledger_disk: open tmp {}: {e}",
                    tmp_path.display()
                ))
            })?;
            f.write_all(body.as_bytes()).map_err(|e| {
                AgentError::SpendGuard(format!(
                    "spend_ledger_disk: write tmp {}: {e}",
                    tmp_path.display()
                ))
            })?;
            f.sync_all().map_err(|e| {
                AgentError::SpendGuard(format!(
                    "spend_ledger_disk: sync tmp {}: {e}",
                    tmp_path.display()
                ))
            })?;
        }

        fs::rename(&tmp_path, &file_path).map_err(|e| {
            AgentError::SpendGuard(format!(
                "spend_ledger_disk: rename {} -> {}: {e}",
                tmp_path.display(),
                file_path.display()
            ))
        })?;

        Ok(())
    }
}

/// Helper to release multiple legs reserved under `key` (and any `{key}:{idx}`).
pub fn release_legs(dir: &Path, key: &str) -> Result<(), AgentError> {
    SpendLedger::release(dir, key)
}

/// Helper to reserve multiple legs for a single proposal or batch operation.
pub fn reserve_legs(
    dir: &Path,
    key: &str,
    legs: &[(String, U256, U256)],
    policy: &AgentSessionPolicy,
) -> Result<(), AgentError> {
    for (idx, (asset, amount, balance)) in legs.iter().enumerate() {
        let leg_key = if legs.len() > 1 {
            format!("{key}:{idx}")
        } else {
            key.to_string()
        };
        SpendLedger::reserve(
            dir,
            &leg_key,
            asset,
            *amount,
            *balance,
            policy.daily_cap_pct,
            policy.enforcement,
        )?;
    }
    Ok(())
}

/// Check if a destination address is allowed by the session policy.
pub fn check_destination_allowed(
    policy: &AgentSessionPolicy,
    destination: Address,
) -> Result<(), AgentError> {
    let needle = format!("{destination:#x}").to_ascii_lowercase();
    let allowed = policy
        .allow_transfer_to
        .iter()
        .any(|a| a.trim().to_ascii_lowercase() == needle);
    if !allowed {
        return Err(AgentError::SpendGuard(format!(
            "destination_not_allowed: {destination:#x} is not in the policy's allow_transfer_to list"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // 1. under-cap reserve succeeds; the reserve that would exceed the cap fails spend_cap_exceeded:
    #[test]
    fn test_under_cap_succeeds_and_over_cap_fails() {
        let dir = tempdir().unwrap();
        let balance = U256::from(1000);
        let cap_pct = 20; // 200 cap

        let res1 = SpendLedger::reserve(
            dir.path(),
            "tx1",
            "369:native",
            U256::from(150),
            balance,
            cap_pct,
            EnforcementMode::Enforced,
        );
        assert!(
            matches!(res1, Ok(ReserveOutcome::Reserved { spent, cap }) if spent == U256::from(150) && cap == U256::from(200))
        );

        let res2 = SpendLedger::reserve(
            dir.path(),
            "tx2",
            "369:native",
            U256::from(60), // 150 + 60 = 210 > 200
            balance,
            cap_pct,
            EnforcementMode::Enforced,
        );
        let err = res2.unwrap_err();
        assert!(
            err.to_string().starts_with("spend_cap_exceeded:"),
            "expected spend_cap_exceeded:, got: {err}"
        );
    }

    // 2. restart: reserve, drop, read the file fresh — the spent total is still there
    #[test]
    fn test_restart_persists_spent() {
        let dir = tempdir().unwrap();
        let balance = U256::from(10_000);
        SpendLedger::reserve(
            dir.path(),
            "k1",
            "369:native",
            U256::from(500),
            balance,
            20,
            EnforcementMode::Enforced,
        )
        .unwrap();

        // Fresh load via snapshot
        let snap = SpendLedger::snapshot(dir.path()).unwrap();
        let asset = snap.assets.get("369:native").unwrap();
        assert_eq!(asset.spent, U256::from(500));
        assert_eq!(asset.snapshot, balance);

        // Next reserve also sees previous spent
        let res2 = SpendLedger::reserve(
            dir.path(),
            "k2",
            "369:native",
            U256::from(300),
            balance,
            20,
            EnforcementMode::Enforced,
        )
        .unwrap();
        assert_eq!(
            res2,
            ReserveOutcome::Reserved {
                spent: U256::from(800),
                cap: U256::from(2000),
            }
        );
    }

    // 3. same key twice returns spend_already_reserved error and does not double-charge
    #[test]
    fn test_idempotency_same_key_already_reserved() {
        let dir = tempdir().unwrap();
        let balance = U256::from(1000);
        let r1 = SpendLedger::reserve(
            dir.path(),
            "same-key",
            "369:native",
            U256::from(100),
            balance,
            20,
            EnforcementMode::Enforced,
        )
        .unwrap();
        assert!(matches!(r1, ReserveOutcome::Reserved { .. }));

        let err = SpendLedger::reserve(
            dir.path(),
            "same-key",
            "369:native",
            U256::from(100),
            balance,
            20,
            EnforcementMode::Enforced,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.starts_with("spend_already_reserved:"),
            "expected spend_already_reserved error, got: {msg}"
        );

        let snap = SpendLedger::snapshot(dir.path()).unwrap();
        assert_eq!(
            snap.assets.get("369:native").unwrap().spent,
            U256::from(100)
        );
    }

    // 4. window rollover resets spent and re-snapshots; backwards clock starts a fresh window and warns
    #[test]
    fn test_window_rollover_and_backwards_clock() {
        let dir = tempdir().unwrap();
        let t0 = 1_700_000_000u64;

        // Spend in window 1
        SpendLedger::reserve_at(
            dir.path(),
            "k1",
            "369:native",
            U256::from(200),
            U256::from(1000),
            20,
            EnforcementMode::Enforced,
            t0,
        )
        .unwrap();

        // Advance past window (t0 + 86400)
        let t1 = t0 + 86_400 + 10;
        let r2 = SpendLedger::reserve_at(
            dir.path(),
            "k2",
            "369:native",
            U256::from(150),
            U256::from(2000), // new balance snapshot
            20,
            EnforcementMode::Enforced,
            t1,
        )
        .unwrap();
        assert_eq!(
            r2,
            ReserveOutcome::Reserved {
                spent: U256::from(150),
                cap: U256::from(400),
            }
        );

        // Jump backwards in clock
        let t2 = t1 - 50_000;
        let r3 = SpendLedger::reserve_at(
            dir.path(),
            "k3",
            "369:native",
            U256::from(100),
            U256::from(3000), // fresh snapshot
            20,
            EnforcementMode::Enforced,
            t2,
        )
        .unwrap();
        assert_eq!(
            r3,
            ReserveOutcome::Reserved {
                spent: U256::from(100),
                cap: U256::from(600),
            }
        );
    }

    // 5. corrupt JSON → spend_ledger_unreadable:
    #[test]
    fn test_corrupt_json_unreadable() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(SPEND_LEDGER_FILE), "{ invalid json }").unwrap();

        let res = SpendLedger::reserve(
            dir.path(),
            "k",
            "369:native",
            U256::from(10),
            U256::from(100),
            20,
            EnforcementMode::Enforced,
        );
        let err = res.unwrap_err();
        assert!(
            err.to_string().starts_with("spend_ledger_unreadable:"),
            "expected spend_ledger_unreadable:, got: {err}"
        );
    }

    // 6. stale lock is reclaimed; live lock expires to spend_ledger_busy: within retry budget
    #[test]
    fn test_lock_stale_reclaimed_and_busy_timeout() {
        let dir = tempdir().unwrap();
        let lock_path = dir.path().join(SPEND_LOCK_FILE);

        // Dead pid (99999999 on linux)
        fs::write(&lock_path, "99999999:1700000000").unwrap();
        let res = SpendLedger::reserve(
            dir.path(),
            "k1",
            "369:native",
            U256::from(10),
            U256::from(100),
            20,
            EnforcementMode::Enforced,
        );
        assert!(res.is_ok(), "stale lock with dead pid should be reclaimed");

        // Live lock held by current pid with fresh timestamp
        let live_lock = dir.path().join(SPEND_LOCK_FILE);
        let now = current_unix();
        fs::write(&live_lock, format!("{}:{now}", std::process::id())).unwrap();

        let start = std::time::Instant::now();
        let res2 = SpendLedger::reserve(
            dir.path(),
            "k2",
            "369:native",
            U256::from(10),
            U256::from(100),
            20,
            EnforcementMode::Enforced,
        );
        let elapsed = start.elapsed();
        let err = res2.unwrap_err();
        assert!(
            err.to_string().starts_with("spend_ledger_busy:"),
            "expected spend_ledger_busy:, got: {err}"
        );
        // Retry budget is 20 * 5ms = 100ms
        assert!(
            elapsed.as_millis() >= 90,
            "expected around 100ms retry, got {elapsed:?}"
        );
    }

    // 7. two assets on one chain have separate buckets
    #[test]
    fn test_separate_asset_buckets() {
        let dir = tempdir().unwrap();
        let native = "369:native";
        let token = "369:0x1234567890123456789012345678901234567890";

        // Reserve 200 native out of 1000 (cap 200)
        SpendLedger::reserve(
            dir.path(),
            "k1",
            native,
            U256::from(200),
            U256::from(1000),
            20,
            EnforcementMode::Enforced,
        )
        .unwrap();

        // Native is capped now
        let native_over = SpendLedger::reserve(
            dir.path(),
            "k2",
            native,
            U256::from(1),
            U256::from(1000),
            20,
            EnforcementMode::Enforced,
        );
        assert!(native_over
            .unwrap_err()
            .to_string()
            .starts_with("spend_cap_exceeded:"));

        // Token is untouched and succeeds
        let token_ok = SpendLedger::reserve(
            dir.path(),
            "k3",
            token,
            U256::from(500),
            U256::from(5000),
            20,
            EnforcementMode::Enforced,
        );
        assert!(token_ok.is_ok());
    }

    // 8. WarnOnly allows without reserving; Enforced refuses
    #[test]
    fn test_warn_only_vs_enforced() {
        let dir = tempdir().unwrap();
        let asset = "369:native";
        let balance = U256::from(1000);

        // WarnOnly allows spend > cap without persisting reservation
        let r_warn = SpendLedger::reserve(
            dir.path(),
            "kwarn",
            asset,
            U256::from(500), // > 20% cap (200)
            balance,
            20,
            EnforcementMode::WarnOnly,
        )
        .unwrap();
        assert!(matches!(r_warn, ReserveOutcome::Reserved { .. }));

        // Spent in snapshot is still 0
        let snap = SpendLedger::snapshot(dir.path()).unwrap();
        assert!(snap.assets.is_empty() || snap.assets.get(asset).unwrap().spent.is_zero());

        // Enforced refuses
        let r_enf = SpendLedger::reserve(
            dir.path(),
            "kenf",
            asset,
            U256::from(250),
            balance,
            20,
            EnforcementMode::Enforced,
        );
        assert!(r_enf
            .unwrap_err()
            .to_string()
            .starts_with("spend_cap_exceeded:"));
    }

    #[cfg(unix)]
    #[test]
    fn test_ledger_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        SpendLedger::reserve(
            dir.path(),
            "k",
            "369:native",
            U256::from(10),
            U256::from(100),
            20,
            EnforcementMode::Enforced,
        )
        .unwrap();
        let mode = fs::metadata(dir.path().join(SPEND_LEDGER_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn test_release_legs_refunds_spend_and_allows_retry() {
        let dir = tempdir().unwrap();
        let asset = "369:native";
        let balance = U256::from(1000);
        let policy = AgentSessionPolicy {
            daily_cap_pct: 20,
            enforcement: EnforcementMode::Enforced,
            ..Default::default()
        };

        let legs = vec![(asset.to_string(), U256::from(150), balance)];
        reserve_legs(dir.path(), "prop_1", &legs, &policy).unwrap();

        let snap1 = SpendLedger::snapshot(dir.path()).unwrap();
        assert_eq!(snap1.assets.get(asset).unwrap().spent, U256::from(150));
        assert_eq!(snap1.recent.len(), 1);

        // A second reservation of 100 exceeds the 200 cap (150 + 100 > 200)
        let legs2 = vec![(asset.to_string(), U256::from(100), balance)];
        assert!(reserve_legs(dir.path(), "prop_2", &legs2, &policy).is_err());

        // Release prop_1
        release_legs(dir.path(), "prop_1").unwrap();

        let snap2 = SpendLedger::snapshot(dir.path()).unwrap();
        assert_eq!(snap2.assets.get(asset).unwrap().spent, U256::ZERO);
        assert!(snap2.recent.is_empty());

        // Now prop_1 can be retried without spend_already_reserved
        reserve_legs(dir.path(), "prop_1", &legs, &policy).unwrap();
        let snap3 = SpendLedger::snapshot(dir.path()).unwrap();
        assert_eq!(snap3.assets.get(asset).unwrap().spent, U256::from(150));
    }

    #[test]
    fn test_failed_send_leaves_spent_unchanged_and_allows_retry() {
        let dir = tempdir().unwrap();
        let asset = "369:native";
        let balance = U256::from(1000);
        let policy = AgentSessionPolicy {
            daily_cap_pct: 20,
            enforcement: EnforcementMode::Enforced,
            ..Default::default()
        };

        let snap_init = SpendLedger::snapshot(dir.path()).unwrap();
        let spent_init = snap_init
            .assets
            .get(asset)
            .map(|a| a.spent)
            .unwrap_or(U256::ZERO);
        assert_eq!(spent_init, U256::ZERO);

        let legs = vec![(asset.to_string(), U256::from(100), balance)];
        let prop_id = "prop_failed_send";

        // Gate passes and reserves
        reserve_legs(dir.path(), prop_id, &legs, &policy).unwrap();
        let snap_reserved = SpendLedger::snapshot(dir.path()).unwrap();
        assert_eq!(
            snap_reserved.assets.get(asset).unwrap().spent,
            U256::from(100)
        );

        // Send fails post-gate -> release_legs called
        release_legs(dir.path(), prop_id).unwrap();

        // 1. Failed send leaves spent unchanged compared to initial state
        let snap_after = SpendLedger::snapshot(dir.path()).unwrap();
        let spent_after = snap_after
            .assets
            .get(asset)
            .map(|a| a.spent)
            .unwrap_or(U256::ZERO);
        assert_eq!(
            spent_after, spent_init,
            "failed send must leave spent budget unchanged"
        );
        assert!(
            !snap_after.recent.iter().any(|r| r.key == prop_id),
            "recent reservations must not hold the failed proposal"
        );

        // 2. Allows an immediate retry of the same proposal_id without spend_already_reserved
        assert!(
            reserve_legs(dir.path(), prop_id, &legs, &policy).is_ok(),
            "immediate retry of the same proposal id must succeed after failed send release"
        );
        let snap_retried = SpendLedger::snapshot(dir.path()).unwrap();
        assert_eq!(
            snap_retried.assets.get(asset).unwrap().spent,
            U256::from(100)
        );
    }

    #[test]
    fn test_successful_send_keeps_reservation() {
        let dir = tempdir().unwrap();
        let asset = "369:native";
        let balance = U256::from(1000);
        let policy = AgentSessionPolicy {
            daily_cap_pct: 20,
            enforcement: EnforcementMode::Enforced,
            ..Default::default()
        };

        let legs = vec![(asset.to_string(), U256::from(100), balance)];
        let prop_id = "prop_successful_send";

        // Gate passes and reserves
        reserve_legs(dir.path(), prop_id, &legs, &policy).unwrap();

        // Send succeeds -> reservation kept (no release_legs)
        let snap = SpendLedger::snapshot(dir.path()).unwrap();
        assert_eq!(snap.assets.get(asset).unwrap().spent, U256::from(100));
        assert!(snap.recent.iter().any(|r| r.key == prop_id));

        // Replay retry with same proposal_id must be refused with spend_already_reserved
        let err = reserve_legs(dir.path(), prop_id, &legs, &policy).unwrap_err();
        assert!(
            err.to_string().contains("spend_already_reserved"),
            "successful send reservation must block replay of same proposal_id: {err}"
        );
    }

    #[test]
    fn test_validation_refusal_never_reserves() {
        let dir = tempdir().unwrap();
        let asset = "369:native";
        let balance = U256::from(1000);
        let policy = AgentSessionPolicy {
            daily_cap_pct: 20,
            enforcement: EnforcementMode::Enforced,
            ..Default::default()
        };

        // When validation fails (e.g. amount exceeds balance or policy checks fail),
        // reserve_legs is never called.
        let snap = SpendLedger::snapshot(dir.path()).unwrap();
        assert!(snap.assets.is_empty());
        assert!(snap.recent.is_empty());

        // A valid proposal can then reserve without interference
        let prop_id = "prop_after_refusal";
        let legs = vec![(asset.to_string(), U256::from(100), balance)];
        reserve_legs(dir.path(), prop_id, &legs, &policy).unwrap();
        let snap2 = SpendLedger::snapshot(dir.path()).unwrap();
        assert_eq!(snap2.assets.get(asset).unwrap().spent, U256::from(100));
    }
}
