//! RAILGUN keys derived from the vault seed.
//!
//! Derivation is railgun-rust's `WalletKeys::from_seed`, which matches the
//! official engine (`m/44'/1984'/0'/0'/i'` spend, `m/420'/1984'/0'/0'/i'`
//! view). The RAILGUN index is the Vaughan HD account index, so `wallet N`
//! and Railway wallet account N share one 0zk address.
//!
//! Only viewing material is kept: it can decrypt notes and spot spends, but
//! can't move funds. The spending key is wiped right after derivation.
//! Residual: upstream copies key bytes internally during derivation and
//! those copies aren't wiped (`docs/railgun-go-no-go.md`).

use alloy::primitives::U256;
use broadcaster_core::crypto::railgun::ViewingKeyData;
use railgun_wallet::keys::WalletKeys;
use vaughan_core::error::WalletError;
use zeroize::Zeroize;

/// Viewing keys for one RAILGUN account. No `Debug`; wiped on drop.
pub struct ViewingKeys(ViewingKeyData);

impl ViewingKeys {
    /// Upstream key data for the log scanner.
    pub(crate) fn scan_keys(&self) -> &ViewingKeyData {
        &self.0
    }

    /// Viewing private key bytes (cache key derivation only).
    pub(crate) fn private_key(&self) -> &[u8; 32] {
        &self.0.viewing_private_key
    }

    /// Key that turns a note position into its nullifier (spent marker).
    pub(crate) fn nullifying_key(&self) -> U256 {
        self.0.nullifying_key
    }
}

impl Drop for ViewingKeys {
    fn drop(&mut self) {
        self.0.viewing_private_key.zeroize();
        self.0.nullifying_key = U256::ZERO;
    }
}

/// A RAILGUN account: its all-chains 0zk address plus viewing keys.
pub struct RailgunAccount {
    hd_index: u32,
    address: String,
    viewing: ViewingKeys,
}

impl RailgunAccount {
    /// Derive account `hd_index` from a BIP-39 seed.
    pub fn from_seed(seed: &[u8; 64], hd_index: u32) -> Result<Self, WalletError> {
        let mut keys = WalletKeys::from_seed(seed, hd_index)
            .map_err(|e| WalletError::KeyDerivationFailed(format!("RAILGUN keys: {e}")))?;
        keys.spending_private_key.zeroize();
        let viewing = ViewingKeys(keys.viewing);
        keys.viewing.viewing_private_key.zeroize();
        let address = viewing
            .0
            .derive_address(None)
            .map_err(|e| WalletError::KeyDerivationFailed(format!("0zk address: {e}")))?
            .to_string();
        Ok(Self {
            hd_index,
            address,
            viewing,
        })
    }

    /// Vaughan HD account index (= RAILGUN derivation index).
    #[must_use]
    pub fn hd_index(&self) -> u32 {
        self.hd_index
    }

    /// All-chains `0zk1…` address (the Railway default format).
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    pub(crate) fn viewing(&self) -> &ViewingKeys {
        &self.viewing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vaughan_core::security::hd_wallet::validate_mnemonic;

    /// Official-engine vectors: `test-vectors/railgun/vectors.json`.
    const VECTORS: &str = include_str!("../../test-vectors/railgun/vectors.json");

    #[test]
    fn matches_official_engine_vectors() {
        let v: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
        let seed = validate_mnemonic(v["mnemonic"].as_str().unwrap())
            .unwrap()
            .to_seed("");
        let accounts = v["accounts"].as_array().unwrap();
        assert!(accounts.len() >= 3);
        for a in accounts {
            let index = u32::try_from(a["index"].as_u64().unwrap()).unwrap();
            let account = RailgunAccount::from_seed(&seed, index).unwrap();
            assert_eq!(
                account.address(),
                a["address"].as_str().unwrap(),
                "index {index}"
            );
            let data = account.viewing().scan_keys();
            assert_eq!(
                format!("{:064x}", data.master_public_key),
                a["masterPublicKey"].as_str().unwrap()
            );
            assert_eq!(
                hex::encode(data.viewing_public_key),
                a["viewingPublicKey"].as_str().unwrap()
            );
        }
    }
}
