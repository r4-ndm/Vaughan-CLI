//! Opt-in RAILGUN shielded balances for Vaughan.
//!
//! Wraps the pinned upstream `railgun-rust` crates (`railgun-wallet`,
//! `broadcaster-core`) behind the `enabled` feature. Without that feature the
//! crate is an empty stub, so the default workspace build never compiles the
//! RAILGUN stack. Frontends turn it on through their own `railgun` feature.
//!
//! Phase 1 is read-only: the 0zk address (`keys`), which chains have
//! RAILGUN (`deployment`), a log scanner (`scan`), balances (`balance`)
//! and an encrypted scan cache (`cache`). No POI calls, no signing.
//! Phase 2 adds `shield` (Sepolia-only ERC-20 shields) and `verify`: the
//! approval card checks a shield pays this wallet.
//!
//! Plan, threat model and pin-bump procedure: `docs/railgun-go-no-go.md`.

#[cfg(feature = "enabled")]
pub mod balance;
#[cfg(feature = "enabled")]
pub mod cache;
#[cfg(feature = "enabled")]
pub mod deployment;
#[cfg(feature = "enabled")]
pub mod keys;
#[cfg(feature = "enabled")]
pub mod quick;
#[cfg(feature = "enabled")]
pub mod scan;
#[cfg(feature = "enabled")]
pub mod shield;
#[cfg(feature = "enabled")]
pub mod verify;

/// Whether this build includes RAILGUN support (the `enabled` feature).
#[must_use]
pub const fn is_enabled() -> bool {
    cfg!(feature = "enabled")
}

/// Derive the RAILGUN account for the active HD account (fast; no KDF).
///
/// Refuses hardware and imported accounts. The seed never leaves
/// [`vaughan_core::core::AccountManager::with_hd_seed`].
#[cfg(feature = "enabled")]
pub fn open_account(
    accounts: &vaughan_core::core::AccountManager,
) -> Result<keys::RailgunAccount, vaughan_core::error::WalletError> {
    accounts.with_hd_seed(keys::RailgunAccount::from_seed)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_flag_matches_feature() {
        assert_eq!(is_enabled(), cfg!(feature = "enabled"));
    }

    #[cfg(feature = "enabled")]
    #[test]
    fn account_follows_active_hd_account_only() {
        use secrecy::SecretString;
        use vaughan_core::core::AccountManager;

        const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        const ANVIL_KEY0: &str =
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let mut am = AccountManager::from_phrase(MNEMONIC, 3).unwrap();
        am.set_active(1).unwrap();
        let account = open_account(&am).unwrap();
        assert_eq!(account.hd_index(), 1);
        assert_eq!(account.address(), scan::tests::account(1).address());

        am.import_private_key("anvil", &SecretString::new(ANVIL_KEY0.into()))
            .unwrap();
        assert!(open_account(&am).is_err());
    }
}
