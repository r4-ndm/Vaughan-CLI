//! Optional Trezor BIP-39 passphrase (hidden wallet).
//!
//! Held only in memory behind [`secrecy::SecretString`]. Never log, display,
//! or write to vault JSON — vault stores watch address + path only.

use secrecy::SecretString;

/// Session-scoped passphrase for the next USB connect (not persisted).
#[derive(Default)]
pub struct TrezorPassphrase {
    inner: Option<SecretString>,
}

impl TrezorPassphrase {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set or replace the passphrase (zeroizes the previous value on drop).
    pub fn set(&mut self, passphrase: SecretString) {
        self.inner = Some(passphrase);
    }

    /// Clear without logging.
    pub fn clear(&mut self) {
        self.inner = None;
    }

    pub fn is_set(&self) -> bool {
        self.inner.is_some()
    }

    /// Borrow for the transport layer only. Prefer dropping the parent after use.
    pub fn secret(&self) -> Option<&SecretString> {
        self.inner.as_ref()
    }
}

impl Drop for TrezorPassphrase {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    #[test]
    fn set_and_clear_roundtrip() {
        let mut p = TrezorPassphrase::new();
        assert!(!p.is_set());
        p.set(SecretString::new("test-passphrase-not-real".into()));
        assert!(p.is_set());
        assert_eq!(
            p.secret().unwrap().expose_secret(),
            "test-passphrase-not-real"
        );
        p.clear();
        assert!(!p.is_set());
    }
}
