//! Account management: HD accounts from a mnemonic plus optional imported keys.

use alloy::signers::local::PrivateKeySigner;
use bip39::Mnemonic;
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroize;

use crate::core::vault_secrets::{
    parse_private_key, private_key_hex, ImportedKeyRecord, VaultSecrets,
};
use crate::error::WalletError;
use crate::security::hd_wallet::{
    derive_account, derive_account_from_parent, derive_account_parent, validate_mnemonic,
};

/// Stable index base for imported keys (HD accounts stay at 0..N-1).
pub const IMPORTED_INDEX_BASE: u32 = 1_000_000;

/// A wallet account: HD-derived or imported private key.
///
/// The address is public; signing material lives only in [`AccountManager`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub index: u32,
    pub address: String,
    /// Display label (`Account 0` or a user-chosen import name).
    pub label: String,
    pub is_imported: bool,
}

struct ImportedKey {
    label: String,
    /// Hex private key; zeroized on drop via [`SecretString`].
    private_key: SecretString,
    address: String,
}

/// Accounts derived from an unlocked mnemonic, plus optional imported EOAs.
///
/// The mnemonic is secret material: this type deliberately implements no
/// `Debug`/`Display`, and the mnemonic is zeroized on drop (via `bip39`).
pub struct AccountManager {
    mnemonic: Mnemonic,
    accounts: Vec<Account>,
    imported: Vec<ImportedKey>,
    active_index: u32,
}

impl AccountManager {
    /// Number of HD accounts derived on unlock by default.
    pub const DEFAULT_ACCOUNT_COUNT: u32 = 10;

    /// Derive `count` HD accounts with account 0 active.
    pub fn new(mnemonic: Mnemonic, count: u32) -> Result<Self, WalletError> {
        Self::with_active(mnemonic, 0, count, Vec::new())
    }

    /// Derive `count` accounts from a mnemonic phrase (validated first).
    pub fn from_phrase(phrase: &str, count: u32) -> Result<Self, WalletError> {
        Self::new(validate_mnemonic(phrase)?, count)
    }

    /// Build from decoded vault secrets (HD + imported).
    pub fn from_secrets(secrets: &VaultSecrets, count: u32) -> Result<Self, WalletError> {
        let mnemonic = validate_mnemonic(&secrets.mnemonic)?;
        let imported = secrets
            .imported
            .iter()
            .map(|rec| {
                let signer = parse_private_key(&rec.private_key)?;
                Ok(ImportedKey {
                    label: if rec.label.trim().is_empty() {
                        "Imported".into()
                    } else {
                        rec.label.clone()
                    },
                    private_key: SecretString::new(rec.private_key.clone()),
                    address: signer.address().to_string(),
                })
            })
            .collect::<Result<Vec<_>, WalletError>>()?;
        Self::with_active(mnemonic, 0, count, imported)
    }

    /// Derive `count` HD accounts with `active_index` active.
    fn with_active(
        mnemonic: Mnemonic,
        active_index: u32,
        count: u32,
        imported: Vec<ImportedKey>,
    ) -> Result<Self, WalletError> {
        let parent = derive_account_parent(&mnemonic)?;
        let mut accounts = Vec::with_capacity(count as usize + imported.len());
        for index in 0..count {
            let signer = derive_account_from_parent(&parent, index)?;
            accounts.push(Account {
                index,
                address: signer.address().to_string(),
                label: format!("Account {index}"),
                is_imported: false,
            });
        }
        for (i, key) in imported.iter().enumerate() {
            accounts.push(Account {
                index: IMPORTED_INDEX_BASE + i as u32,
                address: key.address.clone(),
                label: key.label.clone(),
                is_imported: true,
            });
        }
        let mut am = Self {
            mnemonic,
            accounts,
            imported,
            active_index: 0,
        };
        am.set_active(active_index)?;
        Ok(am)
    }

    fn rebuild_account_list(&mut self) -> Result<(), WalletError> {
        let count = self
            .accounts
            .iter()
            .filter(|a| !a.is_imported)
            .count()
            .max(1) as u32;
        let parent = derive_account_parent(&self.mnemonic)?;
        let mut accounts = Vec::with_capacity(count as usize + self.imported.len());
        for index in 0..count {
            let signer = derive_account_from_parent(&parent, index)?;
            accounts.push(Account {
                index,
                address: signer.address().to_string(),
                label: format!("Account {index}"),
                is_imported: false,
            });
        }
        for (i, key) in self.imported.iter().enumerate() {
            accounts.push(Account {
                index: IMPORTED_INDEX_BASE + i as u32,
                address: key.address.clone(),
                label: key.label.clone(),
                is_imported: true,
            });
        }
        self.accounts = accounts;
        if self.accounts.iter().all(|a| a.index != self.active_index) {
            self.active_index = self.accounts.first().map(|a| a.index).unwrap_or(0);
        }
        Ok(())
    }

    /// Snapshot suitable for re-encrypting the vault.
    pub fn to_secrets(&self) -> VaultSecrets {
        VaultSecrets {
            mnemonic: self.mnemonic.to_string(),
            imported: self
                .imported
                .iter()
                .map(|k| ImportedKeyRecord {
                    label: k.label.clone(),
                    private_key: k.private_key.expose_secret().clone(),
                })
                .collect(),
        }
    }

    /// Import a raw private key (hex). Re-persists via the caller's vault rewrite.
    pub fn import_private_key(
        &mut self,
        label: impl Into<String>,
        private_key: &SecretString,
    ) -> Result<Account, WalletError> {
        let signer = parse_private_key(private_key.expose_secret())?;
        let address = signer.address().to_string();
        if self
            .accounts
            .iter()
            .any(|a| a.address.eq_ignore_ascii_case(&address))
        {
            return Err(WalletError::Other(
                "that address is already in this wallet".to_string(),
            ));
        }
        let label = {
            let l = label.into();
            if l.trim().is_empty() {
                "Imported".into()
            } else {
                l
            }
        };
        self.imported.push(ImportedKey {
            label,
            private_key: SecretString::new(private_key.expose_secret().clone()),
            address: address.clone(),
        });
        self.rebuild_account_list()?;
        let account = self
            .accounts
            .iter()
            .find(|a| a.address.eq_ignore_ascii_case(&address))
            .cloned()
            .ok_or_else(|| WalletError::Other("imported account missing after rebuild".into()))?;
        self.active_index = account.index;
        Ok(account)
    }

    /// BIP-39 phrase for export (caller must gate on password and clear UI).
    pub fn mnemonic_phrase(&self) -> SecretString {
        SecretString::new(self.mnemonic.to_string())
    }

    /// Active account's private key hex for export (password-gated by caller).
    pub fn export_private_key(&self, index: u32) -> Result<SecretString, WalletError> {
        let signer = self.signer(index)?;
        Ok(private_key_hex(&signer))
    }

    /// All accounts (HD then imported).
    pub fn accounts(&self) -> &[Account] {
        &self.accounts
    }

    /// The active account.
    pub fn active_account(&self) -> &Account {
        self.accounts
            .iter()
            .find(|a| a.index == self.active_index)
            .expect("active account must exist")
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
        if !self.accounts.iter().any(|a| a.index == index) {
            return Err(WalletError::AccountNotFound(format!(
                "account index {index}"
            )));
        }
        self.active_index = index;
        Ok(())
    }

    /// Re-derive or load the signing key for `index` (caller drops it when done).
    pub fn signer(&self, index: u32) -> Result<PrivateKeySigner, WalletError> {
        if let Some(account) = self.accounts.iter().find(|a| a.index == index) {
            if account.is_imported {
                let offset = (index - IMPORTED_INDEX_BASE) as usize;
                let key = self.imported.get(offset).ok_or_else(|| {
                    WalletError::AccountNotFound(format!("imported account {index}"))
                })?;
                return parse_private_key(key.private_key.expose_secret());
            }
            return derive_account(&self.mnemonic, index);
        }
        Err(WalletError::AccountNotFound(format!(
            "account index {index}"
        )))
    }

    /// The active account's signing key.
    pub fn active_signer(&self) -> Result<PrivateKeySigner, WalletError> {
        self.signer(self.active_index)
    }

    /// ERC-5564 spend/view keys derived from this vault's mnemonic.
    pub fn stealth_keys(&self) -> Result<crate::security::stealth::StealthMetaKeys, WalletError> {
        crate::security::stealth::StealthMetaKeys::from_mnemonic(&self.mnemonic)
    }
}

impl Drop for AccountManager {
    fn drop(&mut self) {
        for key in &mut self.imported {
            // SecretString zeroizes on drop; clear label too.
            key.label.zeroize();
        }
        self.imported.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const TEST_ADDRESS_0: &str = "0x9858effd232b4033e47d90003d41ec34ecaeda94";
    const ANVIL_KEY0: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

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

    #[test]
    fn import_private_key_adds_account() {
        let mut am = AccountManager::from_phrase(TEST_MNEMONIC, 2).unwrap();
        let account = am
            .import_private_key("anvil", &SecretString::new(ANVIL_KEY0.into()))
            .unwrap();
        assert!(account.is_imported);
        assert_eq!(am.accounts().len(), 3);
        assert_eq!(
            am.active_address().to_lowercase(),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
        let exported = am.export_private_key(account.index).unwrap();
        assert!(exported.expose_secret().eq_ignore_ascii_case(ANVIL_KEY0));
    }
}
