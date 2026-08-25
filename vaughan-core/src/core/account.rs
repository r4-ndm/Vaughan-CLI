//! Account management: HD accounts from a mnemonic, imported keys, and
//! hardware watch records (Phase 0 — no device I/O).

use alloy::signers::local::PrivateKeySigner;
use bip39::Mnemonic;
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroize;

use crate::core::vault_secrets::{
    parse_private_key, private_key_hex, ImportedKeyRecord, VaultSecrets,
};
use crate::error::WalletError;
use crate::security::hardware::{
    AccountKind, HardwareAccountRecord, LocalSignerBackend, HARDWARE_INDEX_BASE,
};
use crate::security::hd_wallet::{
    derive_account, derive_account_from_parent, derive_account_parent, validate_mnemonic,
};

/// Stable index base for imported keys (HD accounts stay at 0..N-1).
pub const IMPORTED_INDEX_BASE: u32 = 1_000_000;

/// A wallet account: HD-derived, imported private key, or hardware watch.
///
/// The address is public; signing material lives only in [`AccountManager`]
/// (hardware accounts have none in-process).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub index: u32,
    pub address: String,
    /// Display label (`wallet 0`, `W1-HD 1`, or `Ledger · EVM · path`).
    pub label: String,
    /// True for hex imports (not HD, not hardware).
    pub is_imported: bool,
    pub kind: AccountKind,
}

struct ImportedKey {
    label: String,
    /// Hex private key; zeroized on drop via [`SecretString`].
    private_key: SecretString,
    address: String,
    /// HD `wallet N` this key was attached under (for `Wn-HD k` naming).
    parent_wallet: u32,
}

/// Accounts derived from an unlocked mnemonic, plus optional imported EOAs
/// and hardware watch records.
///
/// The mnemonic is secret material: this type deliberately implements no
/// `Debug`/`Display`, and the mnemonic is zeroized on drop (via `bip39`).
pub struct AccountManager {
    mnemonic: Mnemonic,
    accounts: Vec<Account>,
    imported: Vec<ImportedKey>,
    hardware: Vec<HardwareAccountRecord>,
    active_index: u32,
}

/// Default label for a seed-derived HD account.
pub fn hd_wallet_label(index: u32) -> String {
    format!("wallet {index}")
}

/// Default label for a private-key-only import under HD `wallet {parent}`.
///
/// `slot` is 1-based among imports attached to that parent (`W1-HD 1`, …).
pub fn imported_hd_label(parent_wallet: u32, slot: u32) -> String {
    format!("W{parent_wallet}-HD {slot}")
}

impl AccountManager {
    /// Number of HD accounts derived on unlock by default.
    pub const DEFAULT_ACCOUNT_COUNT: u32 = 10;

    /// Derive `count` HD accounts with account 0 active.
    pub fn new(mnemonic: Mnemonic, count: u32) -> Result<Self, WalletError> {
        Self::with_active(mnemonic, 0, count, Vec::new(), Vec::new())
    }

    /// Derive `count` accounts from a mnemonic phrase (validated first).
    pub fn from_phrase(phrase: &str, count: u32) -> Result<Self, WalletError> {
        Self::new(validate_mnemonic(phrase)?, count)
    }

    /// Build from decoded vault secrets (HD + imported) and optional hardware watches.
    pub fn from_secrets(secrets: &VaultSecrets, count: u32) -> Result<Self, WalletError> {
        Self::from_secrets_with_hardware(secrets, count, &[])
    }

    /// Like [`Self::from_secrets`], merging persisted hardware watch records.
    pub fn from_secrets_with_hardware(
        secrets: &VaultSecrets,
        count: u32,
        hardware: &[HardwareAccountRecord],
    ) -> Result<Self, WalletError> {
        let mnemonic = validate_mnemonic(&secrets.mnemonic)?;
        let imported = secrets
            .imported
            .iter()
            .map(|rec| {
                let signer = parse_private_key(&rec.private_key)?;
                Ok(ImportedKey {
                    label: rec.label.clone(),
                    private_key: SecretString::new(rec.private_key.clone()),
                    address: signer.address().to_string(),
                    parent_wallet: rec.parent_wallet,
                })
            })
            .collect::<Result<Vec<_>, WalletError>>()?;
        Self::with_active(mnemonic, 0, count, imported, hardware.to_vec())
    }

    /// Derive `count` HD accounts with `active_index` active.
    fn with_active(
        mnemonic: Mnemonic,
        active_index: u32,
        count: u32,
        imported: Vec<ImportedKey>,
        hardware: Vec<HardwareAccountRecord>,
    ) -> Result<Self, WalletError> {
        let mut am = Self {
            mnemonic,
            accounts: Vec::new(),
            imported,
            hardware,
            active_index: 0,
        };
        am.rebuild_account_list_with_hd_count(count)?;
        am.set_active(active_index)?;
        Ok(am)
    }

    /// Replace hardware watch list (e.g. after unlock from [`PersistedState`]).
    pub fn set_hardware(
        &mut self,
        hardware: Vec<HardwareAccountRecord>,
    ) -> Result<(), WalletError> {
        self.hardware = hardware;
        self.rebuild_account_list()
    }

    /// Hardware watch records (persisted separately from the encrypted vault).
    pub fn hardware(&self) -> &[HardwareAccountRecord] {
        &self.hardware
    }

    /// Resolve import display: keep custom labels; empty → `Wn-HD k`.
    fn display_imported_label(key: &ImportedKey, all: &[ImportedKey]) -> String {
        if !key.label.trim().is_empty() {
            return key.label.clone();
        }
        let slot = all
            .iter()
            .filter(|k| k.parent_wallet == key.parent_wallet)
            .position(|k| k.address.eq_ignore_ascii_case(&key.address))
            .map(|i| i as u32 + 1)
            .unwrap_or(1);
        imported_hd_label(key.parent_wallet, slot)
    }

    fn rebuild_account_list(&mut self) -> Result<(), WalletError> {
        let count = self
            .accounts
            .iter()
            .filter(|a| matches!(a.kind, AccountKind::Hd))
            .count()
            .max(1) as u32;
        self.rebuild_account_list_with_hd_count(count)
    }

    fn rebuild_account_list_with_hd_count(&mut self, count: u32) -> Result<(), WalletError> {
        let parent = derive_account_parent(&self.mnemonic)?;
        let mut accounts =
            Vec::with_capacity(count as usize + self.imported.len() + self.hardware.len());
        for index in 0..count {
            let signer = derive_account_from_parent(&parent, index)?;
            accounts.push(Account {
                index,
                address: signer.address().to_string(),
                label: hd_wallet_label(index),
                is_imported: false,
                kind: AccountKind::Hd,
            });
        }
        for (i, key) in self.imported.iter().enumerate() {
            accounts.push(Account {
                index: IMPORTED_INDEX_BASE + i as u32,
                address: key.address.clone(),
                label: Self::display_imported_label(key, &self.imported),
                is_imported: true,
                kind: AccountKind::Imported,
            });
        }
        for (i, hw) in self.hardware.iter().enumerate() {
            accounts.push(Account {
                index: HARDWARE_INDEX_BASE + i as u32,
                address: hw.address.clone(),
                label: hw.display_label(),
                is_imported: false,
                kind: AccountKind::Hardware(hw.clone()),
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
                    parent_wallet: k.parent_wallet,
                })
                .collect(),
        }
    }

    /// Active HD wallet index (`wallet N`), or the import's parent when on an import.
    pub fn active_parent_wallet(&self) -> u32 {
        let active = self.active_account();
        match &active.kind {
            AccountKind::Imported => self
                .imported
                .iter()
                .find(|k| k.address.eq_ignore_ascii_case(&active.address))
                .map(|k| k.parent_wallet)
                .unwrap_or(0),
            AccountKind::Hd => active.index,
            AccountKind::Hardware(_) => 0,
        }
    }

    /// Import a raw private key (hex). Re-persists via the caller's vault rewrite.
    ///
    /// Empty `label` → `W{parent}-HD k` under the current HD wallet.
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
        let parent_wallet = self.active_parent_wallet();
        let label = label.into();
        self.imported.push(ImportedKey {
            label,
            private_key: SecretString::new(private_key.expose_secret().clone()),
            address: address.clone(),
            parent_wallet,
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

    /// Refuse hardware accounts for software-only operations.
    pub fn require_software_account(&self, index: u32) -> Result<(), WalletError> {
        let account = self
            .accounts
            .iter()
            .find(|a| a.index == index)
            .ok_or_else(|| WalletError::AccountNotFound(format!("account index {index}")))?;
        if account.kind.is_hardware() {
            return Err(WalletError::HardwareUnsupported(
                "this account is on a hardware wallet — keys never leave the device".into(),
            ));
        }
        Ok(())
    }

    /// Active account must be software (HD or imported).
    pub fn require_software_active(&self) -> Result<(), WalletError> {
        self.require_software_account(self.active_index)
    }

    /// Active account's private key hex for export (password-gated by caller).
    pub fn export_private_key(&self, index: u32) -> Result<SecretString, WalletError> {
        self.require_software_account(index)?;
        let signer = self.signer(index)?;
        Ok(private_key_hex(&signer))
    }

    /// All accounts (HD, then imported, then hardware).
    pub fn accounts(&self) -> &[Account] {
        &self.accounts
    }

    /// Label for account `index`, if present.
    pub fn label_for(&self, index: u32) -> Option<&str> {
        self.accounts
            .iter()
            .find(|a| a.index == index)
            .map(|a| a.label.as_str())
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
    ///
    /// Hardware accounts return [`WalletError::HardwareUnsupported`].
    pub fn signer(&self, index: u32) -> Result<PrivateKeySigner, WalletError> {
        self.require_software_account(index)?;
        if let Some(account) = self.accounts.iter().find(|a| a.index == index) {
            match &account.kind {
                AccountKind::Imported => {
                    let offset = (index - IMPORTED_INDEX_BASE) as usize;
                    let key = self.imported.get(offset).ok_or_else(|| {
                        WalletError::AccountNotFound(format!("imported account {index}"))
                    })?;
                    return parse_private_key(key.private_key.expose_secret());
                }
                AccountKind::Hd => return derive_account(&self.mnemonic, index),
                AccountKind::Hardware(_) => unreachable!("require_software_account"),
            }
        }
        Err(WalletError::AccountNotFound(format!(
            "account index {index}"
        )))
    }

    /// The active account's signing key (software only).
    pub fn active_signer(&self) -> Result<PrivateKeySigner, WalletError> {
        self.signer(self.active_index)
    }

    /// Local [`LocalSignerBackend`] for the active software account.
    pub fn active_local_backend(&self) -> Result<LocalSignerBackend, WalletError> {
        Ok(LocalSignerBackend::new(self.active_signer()?))
    }

    /// ERC-5564 spend/view keys derived from this vault's mnemonic.
    ///
    /// Refuses when the active account is hardware (stealth stays HD-only).
    pub fn stealth_keys(&self) -> Result<crate::security::stealth::StealthMetaKeys, WalletError> {
        self.require_software_active()?;
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
        assert_eq!(am.active_account().label, "wallet 0");
        assert_eq!(am.accounts()[2].label, "wallet 2");
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
    fn export_private_key_matches_selected_account() {
        let mut am = AccountManager::from_phrase(TEST_MNEMONIC, 3).unwrap();
        am.set_active(2).unwrap();
        let expected = am.active_address().to_lowercase();
        let sk = am.export_private_key(am.active_index()).unwrap();
        let signer = parse_private_key(sk.expose_secret()).unwrap();
        assert_eq!(format!("{}", signer.address()).to_lowercase(), expected);

        am.set_active(0).unwrap();
        let sk0 = am.export_private_key(0).unwrap();
        let s0 = parse_private_key(sk0.expose_secret()).unwrap();
        assert_eq!(
            format!("{}", s0.address()).to_lowercase(),
            am.active_address().to_lowercase()
        );
        assert_ne!(sk.expose_secret(), sk0.expose_secret());
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

    #[test]
    fn empty_import_label_uses_parent_wallet_hd_name() {
        let mut am = AccountManager::from_phrase(TEST_MNEMONIC, 3).unwrap();
        am.set_active(1).unwrap();
        let a = am
            .import_private_key("", &SecretString::new(ANVIL_KEY0.into()))
            .unwrap();
        assert_eq!(a.label, "W1-HD 1");
    }

    #[test]
    fn hardware_watch_refuses_export_and_signer() {
        use crate::security::hardware::{HardwareVendor, HwChainFamily};

        let mut am = AccountManager::from_phrase(TEST_MNEMONIC, 1).unwrap();
        am.set_hardware(vec![HardwareAccountRecord {
            vendor: HardwareVendor::Ledger,
            family: HwChainFamily::Evm,
            derivation_path: "m/44'/60'/0'/0/0".into(),
            network_id: Some("943".into()),
            address: "0x1111111111111111111111111111111111111111".into(),
            label: String::new(),
        }])
        .unwrap();
        assert_eq!(am.accounts().len(), 2);
        let hw_index = am
            .accounts()
            .iter()
            .find(|a| a.kind.is_hardware())
            .map(|a| a.index)
            .unwrap();
        assert!(am
            .accounts()
            .iter()
            .find(|a| a.index == hw_index)
            .unwrap()
            .label
            .contains("Ledger"));
        am.set_active(hw_index).unwrap();
        assert!(matches!(
            am.export_private_key(hw_index),
            Err(WalletError::HardwareUnsupported(_))
        ));
        assert!(matches!(
            am.active_signer(),
            Err(WalletError::HardwareUnsupported(_))
        ));
        assert!(matches!(
            am.stealth_keys(),
            Err(WalletError::HardwareUnsupported(_))
        ));
    }
}
