//! Account management: derive and manage a list of accounts from a mnemonic.

use alloy::signers::local::PrivateKeySigner;
use bip39::Mnemonic;

use crate::error::WalletError;
use crate::security::hd_wallet::{
    derive_account, derive_account_from_parent, derive_account_parent, validate_mnemonic,
};

/// A derived account: its derivation index and checksummed address.
///
/// The address is public; the signing key is never stored here and is
/// re-derived on demand via [`AccountManager::signer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub index: u32,
    pub address: String,
}

/// Accounts derived from an unlocked mnemonic.
///
/// The mnemonic is secret material: this type deliberately implements no
/// `Debug`/`Display`, and the mnemonic is zeroized on drop (via `bip39`).
pub struct AccountManager {
    mnemonic: Mnemonic,
    accounts: Vec<Account>,
    active_index: u32,
}

impl AccountManager {
    /// Number of accounts derived on unlock by default.
    pub const DEFAULT_ACCOUNT_COUNT: u32 = 10;

    /// Derive `count` accounts with account 0 active.
    pub fn new(mnemonic: Mnemonic, count: u32) -> Result<Self, WalletError> {
        Self::with_active(mnemonic, 0, count)
    }

    /// Derive `count` accounts from a mnemonic phrase (validated first).
    pub fn from_phrase(phrase: &str, count: u32) -> Result<Self, WalletError> {
        Self::new(validate_mnemonic(phrase)?, count)
    }

    /// Derive `count` accounts with `active_index` active.
    pub fn with_active(
        mnemonic: Mnemonic,
        active_index: u32,
        count: u32,
    ) -> Result<Self, WalletError> {
        // Derive the hardened parent key once; children reuse it instead of
        // re-running PBKDF2 for every account (~10x faster on unlock).
        let parent = derive_account_parent(&mnemonic)?;
        let mut accounts = Vec::with_capacity(count as usize);
        for index in 0..count {
            let signer = derive_account_from_parent(&parent, index)?;
            accounts.push(Account {
                index,
                address: signer.address().to_string(),
            });
        }
        if (active_index as usize) >= accounts.len() {
            return Err(WalletError::AccountNotFound(format!(
                "account index {active_index}"
            )));
        }
        Ok(Self {
            mnemonic,
            accounts,
            active_index,
        })
    }

    /// All derived accounts.
    pub fn accounts(&self) -> &[Account] {
        &self.accounts
    }

    /// The active account.
    pub fn active_account(&self) -> &Account {
        &self.accounts[self.active_index as usize]
    }

    /// The active account's address.
    pub fn active_address(&self) -> &str {
        &self.active_account().address
    }

    /// The active account index.
    pub fn active_index(&self) -> u32 {
        self.active_index
    }

    /// Switch the active account.
    pub fn set_active(&mut self, index: u32) -> Result<(), WalletError> {
        if (index as usize) >= self.accounts.len() {
            return Err(WalletError::AccountNotFound(format!(
                "account index {index}"
            )));
        }
        self.active_index = index;
        Ok(())
    }

    /// Re-derive the signing key for `index` (caller drops it when done).
    pub fn signer(&self, index: u32) -> Result<PrivateKeySigner, WalletError> {
        if (index as usize) >= self.accounts.len() {
            return Err(WalletError::AccountNotFound(format!(
                "account index {index}"
            )));
        }
        derive_account(&self.mnemonic, index)
    }

    /// The active account's signing key.
    pub fn active_signer(&self) -> Result<PrivateKeySigner, WalletError> {
        self.signer(self.active_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const TEST_ADDRESS_0: &str = "0x9858effd232b4033e47d90003d41ec34ecaeda94";

    #[test]
    fn derives_accounts_and_addresses() {
        let am = AccountManager::from_phrase(TEST_MNEMONIC, 3).unwrap();
        assert_eq!(am.accounts().len(), 3);
        assert_eq!(am.active_index(), 0);
        assert_eq!(am.active_address().to_lowercase(), TEST_ADDRESS_0);
    }

    #[test]
    fn active_account_switch() {
        let mut am = AccountManager::from_phrase(TEST_MNEMONIC, 3).unwrap();
        let account_1 = am.signer(1).unwrap().address().to_string();
        am.set_active(1).unwrap();
        assert_eq!(am.active_address().to_lowercase(), account_1.to_lowercase());
        assert!(am.set_active(99).is_err());
    }

    #[test]
    fn signer_matches_address() {
        let am = AccountManager::from_phrase(TEST_MNEMONIC, 2).unwrap();
        let signer = am.active_signer().unwrap();
        assert_eq!(
            signer.address().to_string().to_lowercase(),
            am.active_address().to_lowercase()
        );
    }

    #[test]
    fn rejects_invalid_phrase() {
        assert!(AccountManager::from_phrase("not a mnemonic", 1).is_err());
    }
}
