//! Encrypted Hyperliquid Agent Keystore (`HL-2`).
//!
//! Follows `Vaughan-CLI`'s non-negotiable security guardrails:
//! - Key material is encrypted at rest with **Argon2id + AES-256-GCM** and
//!   persisted with `0600` Unix permissions.
//! - Passwords and raw key bytes are held behind `secrecy::SecretString` and
//!   `zeroize::Zeroizing`, and never reach `Debug`, `Display`, or log output.
//! - Every agent keystore is bound to a dedicated `backing_account` (the
//!   isolated risk-capital sub-account / EOA) and `is_mainnet` flag so an agent
//!   key can never sign for the wrong account or network.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use argon2::{Algorithm, Argon2, Params, Version};
use rand::rngs::OsRng;
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::HlError;
use crate::wire::{sign_l1_action, HlSignature, L1ActionWire};

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

#[cfg_attr(test, allow(dead_code))]
const PROD_M_COST_KIB: u32 = 65_536;
#[cfg_attr(test, allow(dead_code))]
const PROD_T_COST: u32 = 3;
#[cfg_attr(test, allow(dead_code))]
const PROD_P_COST: u32 = 4;

type GcmNonce = Nonce<<Aes256Gcm as AeadCore>::NonceSize>;

#[allow(deprecated)]
fn nonce_from_bytes(bytes: &[u8]) -> GcmNonce {
    *GcmNonce::from_slice(bytes)
}

fn kdf_params() -> Params {
    #[cfg(test)]
    let (m, t, p) = (1_024, 1, 1);
    #[cfg(not(test))]
    let (m, t, p) = (PROD_M_COST_KIB, PROD_T_COST, PROD_P_COST);

    Params::new(m, t, p, Some(KEY_LEN)).expect("valid Argon2id parameters")
}

/// Enforce password policy (>= 8 characters).
pub fn validate_password_policy(password: &SecretString) -> Result<(), HlError> {
    let p = password.expose_secret();
    if p.chars().count() < 8 {
        return Err(HlError::Keystore(
            "password must be at least 8 characters".to_string(),
        ));
    }
    Ok(())
}

/// Construct canonical AES-256-GCM Additional Authenticated Data (AAD) binding
/// the ciphertext to `(agent_address, backing_account, is_mainnet)`.
///
/// Any tampering with `backing_account` or `is_mainnet` in the JSON keystore file
/// will immediately fail AES-256-GCM tag verification on unlock.
fn keystore_aad(agent_address: Address, backing_account: Address, is_mainnet: bool) -> Vec<u8> {
    format!(
        "vaughan-hl-agent-v1|agent:{agent_address:#x}|backing:{backing_account:#x}|mainnet:{is_mainnet}"
    )
    .into_bytes()
}

/// Network-bound unlocked Hyperliquid Agent Signer.
///
/// Refuses to sign if the caller's `is_mainnet` target does not match the
/// keystore's cryptographic network binding, and redacts the inner key from `Debug`.
pub struct UnlockedAgentSigner {
    signer: PrivateKeySigner,
    agent_address: Address,
    backing_account: Address,
    is_mainnet: bool,
}

impl fmt::Debug for UnlockedAgentSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnlockedAgentSigner")
            .field("agent_address", &self.agent_address)
            .field("backing_account", &self.backing_account)
            .field("is_mainnet", &self.is_mainnet)
            .field("signer", &"[REDACTED]")
            .finish()
    }
}

impl UnlockedAgentSigner {
    pub const fn agent_address(&self) -> Address {
        self.agent_address
    }

    pub const fn backing_account(&self) -> Address {
        self.backing_account
    }

    pub const fn is_mainnet(&self) -> bool {
        self.is_mainnet
    }

    /// Sign an L1 action after verifying that `target_mainnet` matches the
    /// keystore's network binding (`Testnet` vs `Mainnet`).
    pub fn sign_l1_action_checked(
        &self,
        action: &L1ActionWire,
        nonce: u64,
        vault_address: Option<Address>,
        target_mainnet: bool,
    ) -> Result<HlSignature, HlError> {
        if target_mainnet != self.is_mainnet {
            return Err(HlError::Signing(format!(
                "network mismatch: keystore is bound to {}, but caller requested {}",
                if self.is_mainnet {
                    "Mainnet"
                } else {
                    "Testnet"
                },
                if target_mainnet { "Mainnet" } else { "Testnet" }
            )));
        }
        sign_l1_action(&self.signer, action, nonce, vault_address, self.is_mainnet)
    }
}

/// On-disk encrypted Hyperliquid Agent Keystore (`0600` permissions).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedAgentKeystore {
    /// Public address of the generated HL Agent Wallet (cannot withdraw; trades only).
    pub agent_address: Address,
    /// Public address of the dedicated risk-capital sub-account / EOA that approves this agent.
    pub backing_account: Address,
    /// Network binding (`false` = Testnet, `true` = Mainnet).
    pub is_mainnet: bool,
    /// Hex-encoded 16-byte Argon2id salt.
    pub salt: String,
    /// Hex-encoded 12-byte AES-256-GCM nonce.
    pub nonce: String,
    /// Hex-encoded AES-256-GCM ciphertext of the 32-byte agent private key.
    pub ciphertext: String,
}

impl EncryptedAgentKeystore {
    /// Generate a fresh CSPRNG (`OsRng`) Hyperliquid Agent Wallet and encrypt it
    /// under `password` (Argon2id -> AES-256-GCM).
    pub fn generate_random(
        backing_account: Address,
        is_mainnet: bool,
        password: &SecretString,
    ) -> Result<Self, HlError> {
        validate_password_policy(password)?;
        if backing_account == Address::ZERO {
            return Err(HlError::Keystore(
                "backing_account must be a non-zero dedicated sub-account/EOA address".to_string(),
            ));
        }

        let mut raw_secret = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(raw_secret.as_mut_slice());
        let signer = PrivateKeySigner::from_bytes(&(*raw_secret).into())
            .map_err(|e| HlError::Keystore(format!("failed to derive agent signer: {e}")))?;
        let agent_address = signer.address();

        let mut salt = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);

        let mut derived_key = Zeroizing::new([0u8; KEY_LEN]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, kdf_params())
            .hash_password_into(
                password.expose_secret().as_bytes(),
                &salt,
                derived_key.as_mut_slice(),
            )
            .map_err(|e| HlError::Keystore(format!("Argon2id KDF failed: {e}")))?;

        let cipher = Aes256Gcm::new_from_slice(derived_key.as_slice())
            .map_err(|e| HlError::Keystore(format!("AES-256-GCM init failed: {e}")))?;
        let nonce = nonce_from_bytes(&nonce_bytes);
        let aad = keystore_aad(agent_address, backing_account, is_mainnet);
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: raw_secret.as_ref(),
                    aad: &aad,
                },
            )
            .map_err(|_| HlError::Keystore("AES-256-GCM encryption failed".to_string()))?;

        Ok(Self {
            agent_address,
            backing_account,
            is_mainnet,
            salt: hex::encode(salt),
            nonce: hex::encode(nonce_bytes),
            ciphertext: hex::encode(ciphertext),
        })
    }

    /// Decrypt the agent private key into a network-bound [`UnlockedAgentSigner`].
    ///
    /// Verifies the AES-256-GCM AAD tag over `(agent_address, backing_account, is_mainnet)`
    /// and zeroizes all intermediate key material before returning.
    pub fn unlock(&self, password: &SecretString) -> Result<UnlockedAgentSigner, HlError> {
        if self.backing_account == Address::ZERO || self.agent_address == Address::ZERO {
            return Err(HlError::Keystore(
                "keystore addresses cannot be zero".to_string(),
            ));
        }
        let salt = hex::decode(&self.salt)
            .map_err(|_| HlError::Keystore("corrupted keystore salt".to_string()))?;
        let nonce_bytes = hex::decode(&self.nonce)
            .map_err(|_| HlError::Keystore("corrupted keystore nonce".to_string()))?;
        let ciphertext = hex::decode(&self.ciphertext)
            .map_err(|_| HlError::Keystore("corrupted keystore ciphertext".to_string()))?;

        if salt.len() != SALT_LEN || nonce_bytes.len() != NONCE_LEN {
            return Err(HlError::Keystore(
                "invalid keystore salt/nonce length".to_string(),
            ));
        }

        let mut derived_key = Zeroizing::new([0u8; KEY_LEN]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, kdf_params())
            .hash_password_into(
                password.expose_secret().as_bytes(),
                &salt,
                derived_key.as_mut_slice(),
            )
            .map_err(|e| HlError::Keystore(format!("Argon2id KDF failed: {e}")))?;

        let cipher = Aes256Gcm::new_from_slice(derived_key.as_slice())
            .map_err(|e| HlError::Keystore(format!("AES-256-GCM init failed: {e}")))?;
        let nonce = nonce_from_bytes(&nonce_bytes);
        let aad = keystore_aad(self.agent_address, self.backing_account, self.is_mainnet);
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    &nonce,
                    Payload {
                        msg: ciphertext.as_ref(),
                        aad: &aad,
                    },
                )
                .map_err(|_| {
                    HlError::Keystore(
                        "decryption failed (wrong password or tampered keystore header)"
                            .to_string(),
                    )
                })?,
        );

        if plaintext.len() != KEY_LEN {
            return Err(HlError::Keystore(
                "decrypted key is not 32 bytes".to_string(),
            ));
        }
        let mut key_bytes = Zeroizing::new([0u8; KEY_LEN]);
        key_bytes.copy_from_slice(plaintext.as_slice());

        let signer = PrivateKeySigner::from_bytes(&(*key_bytes).into())
            .map_err(|_| HlError::Keystore("invalid secp256k1 secret key".to_string()))?;

        if signer.address() != self.agent_address {
            return Err(HlError::Keystore(
                "unlocked signer address does not match keystore header".to_string(),
            ));
        }
        Ok(UnlockedAgentSigner {
            signer,
            agent_address: self.agent_address,
            backing_account: self.backing_account,
            is_mainnet: self.is_mainnet,
        })
    }

    /// Save the encrypted keystore to disk atomically with strict `0600` Unix permissions
    /// at file creation time (no TOCTOU window) and symlink refusal.
    pub fn save_to_file(&self, path: &Path) -> Result<(), HlError> {
        let parent = path.parent().ok_or_else(|| {
            HlError::Keystore("keystore path must have a parent directory".to_string())
        })?;
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }

        if let Ok(meta) = fs::symlink_metadata(path) {
            if meta.file_type().is_symlink() {
                return Err(HlError::Keystore(
                    "refusing to write keystore to a symlink path".to_string(),
                ));
            }
        }

        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        if let Ok(meta) = fs::symlink_metadata(&tmp) {
            if meta.file_type().is_symlink() {
                return Err(HlError::Keystore(
                    "refusing to write keystore temp file to a symlink path".to_string(),
                ));
            }
        }

        let json = serde_json::to_string_pretty(self)?;
        {
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut file = opts.open(&tmp)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        }
        fs::rename(&tmp, path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// Load an encrypted keystore from disk, refusing symlinks.
    pub fn load_from_file(path: &Path) -> Result<Self, HlError> {
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            return Err(HlError::Keystore(
                "refusing to load keystore from a symlink path".to_string(),
            ));
        }
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }
}

/// High-level one-click setup helper: generates an isolated random agent keystore,
/// saves it atomically with 0600 permissions, and returns both the keystore
/// and the EIP-712 typed-data authorization payload ready for the backing wallet to sign.
pub fn generate_and_save_agent_keystore(
    data_dir: &Path,
    backing_account: Address,
    is_mainnet: bool,
    password: &SecretString,
    agent_name: Option<&str>,
    nonce: u64,
) -> Result<(EncryptedAgentKeystore, serde_json::Value), HlError> {
    let ks = EncryptedAgentKeystore::generate_random(backing_account, is_mainnet, password)?;
    let ks_path = data_dir.join("hl_agent_keystore.json");
    ks.save_to_file(&ks_path)?;
    let approve_json = crate::wire::build_approve_agent_typed_data(
        ks.agent_address,
        agent_name,
        nonce,
        is_mainnet,
    );
    Ok((ks, approve_json))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keystore_roundtrip_and_wrong_password_rejected() {
        let pw = SecretString::new("VaughanHl#2026!".to_string());
        let wrong = SecretString::new("WrongPass#2026!".to_string());
        let backing = Address::repeat_byte(0x42);

        let ks = EncryptedAgentKeystore::generate_random(backing, false, &pw).unwrap();
        let unlocked = ks.unlock(&pw).unwrap();
        assert_eq!(unlocked.agent_address(), ks.agent_address);
        assert_eq!(unlocked.backing_account(), backing);
        assert!(!unlocked.is_mainnet());
        assert!(ks.unlock(&wrong).is_err());
    }

    #[test]
    fn keystore_aad_rejects_tampered_network_or_backing_account() {
        let pw = SecretString::new("VaughanHl#2026!".to_string());
        let backing = Address::repeat_byte(0x42);
        let ks = EncryptedAgentKeystore::generate_random(backing, false, &pw).unwrap();

        // Tamper with is_mainnet (Testnet -> Mainnet escalation attempt)
        let mut tampered_net = ks.clone();
        tampered_net.is_mainnet = true;
        assert!(
            tampered_net.unlock(&pw).is_err(),
            "AAD must reject tampered is_mainnet flag"
        );

        // Tamper with backing_account
        let mut tampered_acct = ks;
        tampered_acct.backing_account = Address::repeat_byte(0x99);
        assert!(
            tampered_acct.unlock(&pw).is_err(),
            "AAD must reject tampered backing_account"
        );
    }

    #[test]
    fn unlocked_signer_blocks_cross_network_signing_and_redacts_debug() {
        use crate::wire::{L1ActionWire, OrderGrouping, OrderTypeWire, OrderWire, WireTif};

        let pw = SecretString::new("VaughanHl#2026!".to_string());
        let backing = Address::repeat_byte(0x42);
        let ks = EncryptedAgentKeystore::generate_random(backing, false, &pw).unwrap();
        let unlocked = ks.unlock(&pw).unwrap();

        let dbg = format!("{unlocked:?}");
        assert!(dbg.contains("[REDACTED]"));

        let action = L1ActionWire::Order {
            orders: vec![OrderWire {
                a: 0,
                b: true,
                p: "77400".to_string(),
                s: "0.01".to_string(),
                r: false,
                t: OrderTypeWire::Limit { tif: WireTif::Ioc },
                c: None,
            }],
            grouping: OrderGrouping::Na,
            builder: None,
        };

        // Testnet signing succeeds on a Testnet keystore
        assert!(unlocked
            .sign_l1_action_checked(&action, 1_730_000_000_000, None, false)
            .is_ok());

        // Mainnet signing MUST be blocked on a Testnet keystore
        assert!(unlocked
            .sign_l1_action_checked(&action, 1_730_000_000_000, None, true)
            .is_err());
    }

    #[test]
    fn atomic_save_enforces_0600_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let ks_path = dir.path().join("sub/hl_agent_keystore.json");
        let pw = SecretString::new("VaughanHl#2026!".to_string());
        let ks = EncryptedAgentKeystore::generate_random(Address::repeat_byte(0x11), false, &pw)
            .unwrap();
        ks.save_to_file(&ks_path).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&ks_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "keystore file must be 0600");
        }

        let loaded = EncryptedAgentKeystore::load_from_file(&ks_path).unwrap();
        assert_eq!(loaded.agent_address, ks.agent_address);
    }
}
