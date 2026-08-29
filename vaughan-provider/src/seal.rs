//! Page-origin seal verification (VB extension attestation).
//!
//! The dApp-browser extension proves a `vaughan_page_origin` assertion is
//! theirs by sealing the origin string with AES-256-GCM under a per-launch
//! extension secret (`vaughan_origin_seal` = hex `iv || ciphertext`). The
//! handshake `Origin` header alone is forgeable by any local process that
//! learns the bridge token; the seal key lives only in the per-launch
//! extension bundle (0700 tmpdir) and `vb.session` (0600), and rotates on
//! every VB launch.
//!
//! AES-GCM (allowlisted `aes-gcm` crate) is used instead of HMAC so no new
//! dependency is required; a fresh random IV per message avoids nonce reuse.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};

/// IV length for AES-GCM (96-bit nonces, WebCrypto default).
const IV_LEN: usize = 12;

/// Verify `seal_hex` decrypts to exactly `expected_origin` under `key_hex`.
///
/// Fail-closed: any malformed input, wrong length, or decrypt failure is
/// false. Never logs key or seal material.
pub fn verify_origin_seal(key_hex: &str, seal_hex: &str, expected_origin: &str) -> bool {
    let Ok(key_bytes) = hex::decode(key_hex) else {
        return false;
    };
    if key_bytes.len() != 32 {
        return false;
    }
    let Ok(seal) = hex::decode(seal_hex) else {
        return false;
    };
    if seal.len() <= IV_LEN {
        return false;
    }
    let (iv, ct) = seal.split_at(IV_LEN);
    let Ok(cipher) = Aes256Gcm::new_from_slice(&key_bytes) else {
        return false;
    };
    let Ok(plain) = cipher.decrypt(nonce_from_bytes(iv), ct) else {
        return false;
    };
    plain == expected_origin.as_bytes()
}

/// `#[allow(deprecated)]`: aes-gcm 0.10 pins generic-array 0.14, whose
/// `from_slice` is deprecated in favor of 1.x (which aes-gcm cannot adopt yet).
#[allow(deprecated)]
fn nonce_from_bytes(bytes: &[u8]) -> &Nonce<<Aes256Gcm as aes_gcm::aead::AeadCore>::NonceSize> {
    Nonce::from_slice(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;
    use rand::RngCore;

    fn seal(key: &[u8; 32], origin: &str) -> String {
        let cipher = Aes256Gcm::new_from_slice(key).unwrap();
        let mut iv = [0u8; IV_LEN];
        OsRng.fill_bytes(&mut iv);
        let ct = cipher
            .encrypt(nonce_from_bytes(&iv), origin.as_bytes())
            .unwrap();
        let mut blob = iv.to_vec();
        blob.extend_from_slice(&ct);
        hex::encode(blob)
    }

    #[test]
    fn roundtrip_accepts_matching_origin() {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        let key_hex = hex::encode(key);
        let seal = seal(&key, "https://app.pulsex.com");
        assert!(verify_origin_seal(
            &key_hex,
            &seal,
            "https://app.pulsex.com"
        ));
    }

    #[test]
    fn rejects_wrong_origin_key_and_garbage() {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        let key_hex = hex::encode(key);
        let seal = seal(&key, "https://app.pulsex.com");
        assert!(!verify_origin_seal(&key_hex, &seal, "https://evil.com"));
        let mut other = [0u8; 32];
        OsRng.fill_bytes(&mut other);
        assert!(!verify_origin_seal(
            &hex::encode(other),
            &seal,
            "https://app.pulsex.com"
        ));
        assert!(!verify_origin_seal(
            &key_hex,
            "zz",
            "https://app.pulsex.com"
        ));
        assert!(!verify_origin_seal(
            &key_hex,
            "00",
            "https://app.pulsex.com"
        ));
        assert!(!verify_origin_seal(
            "nothex",
            &seal,
            "https://app.pulsex.com"
        ));
        assert!(!verify_origin_seal(&key_hex, "", "https://app.pulsex.com"));
    }
}
