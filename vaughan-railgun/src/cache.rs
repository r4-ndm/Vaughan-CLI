//! Encrypted on-disk scan cache (notes + checkpoint), so unlocks don't rescan.
//!
//! The vault password isn't kept after unlock, so the key comes from the
//! account's RAILGUN viewing key: Argon2id (vault cost parameters) with a
//! fixed domain-separation salt, then AES-256-GCM. The viewing key can already
//! decrypt every note in the cache from chain data, so this adds no new
//! exposure, needs no second seed access, and lets the KDF run off the UI
//! thread. The chain id, account index and
//! RAILGUN contract are bound in as associated data, so a file can't be
//! replayed onto another account or chain. Never written in plaintext; files
//! live under the profile directory with owner-only permissions.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use broadcaster_core::deployment::RailgunDeployment;
use railgun_wallet::wallet_cache::{deserialize_wallet_utxo, serialize_wallet_utxo};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use vaughan_core::error::WalletError;
use zeroize::Zeroizing;

use crate::keys::RailgunAccount;
use crate::scan::ScanState;

const DOMAIN: &str = "vaughan-railgun-cache-v1";
const FORMAT_VERSION: u32 = 1;
const NONCE_LEN: usize = 12;

/// AES-256 key for the scan cache. No `Debug`; wiped on drop.
pub struct CacheKey(Zeroizing<[u8; 32]>);

impl CacheKey {
    /// Derive the cache key for `account` (Argon2id at vault cost: run it
    /// off the UI thread).
    pub fn derive(account: &RailgunAccount) -> Result<Self, WalletError> {
        Self::from_secret(account.viewing().private_key())
    }

    fn from_secret(secret: &[u8]) -> Result<Self, WalletError> {
        let mut key = Zeroizing::new([0u8; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, kdf_params())
            .hash_password_into(secret, DOMAIN.as_bytes(), key.as_mut_slice())
            .map_err(|e| WalletError::EncryptionFailed(format!("RAILGUN cache key: {e}")))?;
        Ok(Self(key))
    }
}

/// Vault-strength Argon2id costs, with a fast preset for unit tests only.
fn kdf_params() -> Params {
    #[cfg(test)]
    let (m, t, p) = (1_024, 1, 1);
    #[cfg(not(test))]
    let (m, t, p) = (65_536, 3, 4);
    Params::new(m, t, p, Some(32)).expect("valid Argon2id parameters")
}

/// Cache file for `chain_id` / `hd_index` under a profile directory.
#[must_use]
pub fn cache_path(profile_dir: &Path, chain_id: u64, hd_index: u32) -> PathBuf {
    profile_dir
        .join("railgun")
        .join(format!("{chain_id}-{hd_index}.cache"))
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    nonce: String,
    ciphertext: String,
}

#[derive(Serialize, Deserialize)]
struct Plain {
    start_block: u64,
    next_block: u64,
    /// railgun-rust's own note encoding, hex.
    utxos: Vec<String>,
}

fn associated_data(deployment: &RailgunDeployment, hd_index: u32) -> String {
    format!(
        "{DOMAIN}:{}:{hd_index}:{:#x}",
        deployment.chain_id, deployment.contract
    )
}

/// `from_slice` is deprecated by generic-array 0.14, which aes-gcm 0.10 pins.
#[allow(deprecated)]
fn gcm_nonce(bytes: &[u8]) -> Nonce<aes_gcm::aead::consts::U12> {
    *Nonce::from_slice(bytes)
}

fn cipher(key: &CacheKey) -> Result<Aes256Gcm, WalletError> {
    Aes256Gcm::new_from_slice(key.0.as_slice())
        .map_err(|e| WalletError::EncryptionFailed(e.to_string()))
}

/// Encrypt and atomically write `state` to `path`.
pub fn save(
    path: &Path,
    key: &CacheKey,
    deployment: &RailgunDeployment,
    hd_index: u32,
    state: &ScanState,
) -> Result<(), WalletError> {
    let utxos = state
        .utxos()
        .iter()
        .map(|u| serialize_wallet_utxo(u).map(hex::encode))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| WalletError::Serialization(format!("RAILGUN note: {e}")))?;
    let plain = Zeroizing::new(
        serde_json::to_vec(&Plain {
            start_block: state.start_block,
            next_block: state.next_block,
            utxos,
        })
        .map_err(|e| WalletError::Serialization(e.to_string()))?,
    );

    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let aad = associated_data(deployment, hd_index);
    let ciphertext = cipher(key)?
        .encrypt(
            &gcm_nonce(&nonce),
            Payload {
                msg: plain.as_slice(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|e| WalletError::EncryptionFailed(e.to_string()))?;
    let body = serde_json::to_vec(&Envelope {
        version: FORMAT_VERSION,
        nonce: hex::encode(nonce),
        ciphertext: hex::encode(ciphertext),
    })
    .map_err(|e| WalletError::Serialization(e.to_string()))?;
    write_private(path, &body)
}

/// Read and decrypt the cache at `path`. `Ok(None)` when there is none yet.
///
/// A file that doesn't decrypt (other seed, tampering, other chain) is an
/// error; callers fall back to a full rescan.
pub fn load(
    path: &Path,
    key: &CacheKey,
    deployment: &RailgunDeployment,
    hd_index: u32,
) -> Result<Option<ScanState>, WalletError> {
    let body = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(WalletError::Io(e.to_string())),
    };
    let unreadable = || WalletError::DecryptionFailed("RAILGUN cache is unreadable".into());
    let env: Envelope = serde_json::from_slice(&body).map_err(|_| unreadable())?;
    if env.version != FORMAT_VERSION {
        return Err(unreadable());
    }
    let nonce = hex::decode(&env.nonce).map_err(|_| unreadable())?;
    if nonce.len() != NONCE_LEN {
        return Err(unreadable());
    }
    let ciphertext = hex::decode(&env.ciphertext).map_err(|_| unreadable())?;
    let aad = associated_data(deployment, hd_index);
    let plain = Zeroizing::new(
        cipher(key)?
            .decrypt(
                &gcm_nonce(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| unreadable())?,
    );
    let plain: Plain = serde_json::from_slice(&plain).map_err(|_| unreadable())?;
    let utxos = plain
        .utxos
        .iter()
        .map(|h| {
            let bytes = Zeroizing::new(hex::decode(h).map_err(|_| unreadable())?);
            deserialize_wallet_utxo(&bytes).map_err(|_| unreadable())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut state = ScanState::new(deployment);
    state.start_block = plain.start_block;
    state.next_block = plain.next_block.max(plain.start_block);
    state.utxos = utxos;
    Ok(Some(state))
}

/// Delete the cache at `path` (missing file is fine).
pub fn remove(path: &Path) -> Result<(), WalletError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(WalletError::Io(e.to_string())),
    }
}

/// Write via a temp file + rename, owner-only on Unix.
fn write_private(path: &Path, body: &[u8]) -> Result<(), WalletError> {
    let io = |e: std::io::Error| WalletError::Io(e.to_string());
    let dir = path
        .parent()
        .ok_or_else(|| WalletError::Io("RAILGUN cache path has no parent".into()))?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir).map_err(io)?;

    let tmp = path.with_extension("cache.tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&tmp).map_err(io)?;
    file.write_all(body).map_err(io)?;
    file.sync_all().map_err(io)?;
    drop(file);
    fs::rename(&tmp, path).map_err(io)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::balance::balances;
    use crate::deployment::{for_chain, SEPOLIA_CHAIN_ID};
    use crate::scan::tests::{account, shield_log};

    fn state_with_note(deployment: &RailgunDeployment) -> ScanState {
        let me = account(0);
        let mut state = ScanState::new(deployment);
        let log = shield_log(&me, 77, 0, 5_800_000);
        let delta = railgun_wallet::scan::parse_wallet_delta_from_logs(
            &[log],
            &HashMap::from([(5_800_000, 1)]),
            me.viewing().scan_keys(),
        )
        .unwrap();
        state.apply(delta, me.viewing().nullifying_key());
        state.next_block = 5_800_001;
        state
    }

    fn key(byte: u8) -> CacheKey {
        CacheKey::from_secret(&[byte; 32]).unwrap()
    }

    #[test]
    fn key_differs_per_account() {
        let dir = tempfile::tempdir().unwrap();
        let d = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let path = cache_path(dir.path(), d.chain_id, 0);
        save(
            &path,
            &CacheKey::derive(&account(0)).unwrap(),
            &d,
            0,
            &ScanState::new(&d),
        )
        .unwrap();
        assert!(load(&path, &CacheKey::derive(&account(0)).unwrap(), &d, 0)
            .unwrap()
            .is_some());
        assert!(load(&path, &CacheKey::derive(&account(1)).unwrap(), &d, 0).is_err());
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let d = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let path = cache_path(dir.path(), d.chain_id, 0);
        assert!(load(&path, &key(1), &d, 0).unwrap().is_none());

        let state = state_with_note(&d);
        save(&path, &key(1), &d, 0, &state).unwrap();
        let back = load(&path, &key(1), &d, 0).unwrap().unwrap();
        assert_eq!(back.next_block(), 5_800_001);
        assert_eq!(balances(&back), balances(&state));

        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("5800001"), "checkpoint must not be plaintext");
    }

    #[test]
    fn refuses_other_key_account_or_chain() {
        let dir = tempfile::tempdir().unwrap();
        let d = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let path = cache_path(dir.path(), d.chain_id, 0);
        save(&path, &key(1), &d, 0, &state_with_note(&d)).unwrap();

        assert!(load(&path, &key(2), &d, 0).is_err());
        assert!(load(&path, &key(1), &d, 1).is_err());
        assert!(load(&path, &key(1), &for_chain(1).unwrap(), 0).is_err());
    }

    #[test]
    fn detects_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let d = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let path = cache_path(dir.path(), d.chain_id, 0);
        save(&path, &key(1), &d, 0, &state_with_note(&d)).unwrap();
        let mut env: Envelope = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut ct = hex::decode(&env.ciphertext).unwrap();
        ct[0] ^= 1;
        env.ciphertext = hex::encode(ct);
        fs::write(&path, serde_json::to_vec(&env).unwrap()).unwrap();
        assert!(load(&path, &key(1), &d, 0).is_err());

        remove(&path).unwrap();
        remove(&path).unwrap();
        assert!(load(&path, &key(1), &d, 0).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let d = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let path = cache_path(dir.path(), d.chain_id, 0);
        save(&path, &key(1), &d, 0, &ScanState::new(&d)).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
    }
}
