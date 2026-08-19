//! ERC-5564 scheme-1 stealth addresses (secp256k1 + view tags).
//!
//! Spec freeze: `docs/erc-5564-stealth.md`.
//! `h(s)` is keccak256 of the **SEC1 compressed** shared point (33 bytes), matching
//! ScopeLift `stealth-address-sdk` scheme 1 (`@noble/secp256k1` `getSharedSecret`).
//! Spend/view keys: `m/5564'/60'/0'/0'` and `m/5564'/60'/0'/1'`.
//!
//! Spec-first in `vaughan-core`. Does **not** use Kohaku.

use std::str::FromStr;

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use alloy::sol_types::{SolCall, SolEvent};
use bip32::{DerivationPath, XPrv};
use bip39::Mnemonic;
use k256::ecdsa::SigningKey;
use k256::elliptic_curve::bigint::U256 as FieldU256;
use k256::elliptic_curve::ops::Reduce;
use k256::elliptic_curve::sec1::{FromEncodedPoint, ToEncodedPoint};
use k256::elliptic_curve::Group;
use k256::{AffinePoint, EncodedPoint, ProjectivePoint, PublicKey, Scalar};

use crate::error::WalletError;

/// ERC-5564 scheme id for secp256k1 + view tags.
pub const SCHEME_ID: u64 = 1;

/// Canonical ERC-5564 announcer (CREATE2, same address on every chain).
pub const ERC5564_ANNOUNCER: Address =
    alloy::primitives::address!("0x55649E01B5Df198D18D95b5cc5051630cfD45564");

/// Canonical ERC-6538 stealth meta-address registry (CREATE2).
pub const ERC6538_REGISTRY: Address =
    alloy::primitives::address!("0x6538E6bf4B0eBd30A8Ea093027Ac2422ce5d6538");

/// Arachnid deterministic-deployment proxy (CREATE2 factory).
pub const CREATE2_DEPLOYER: Address =
    alloy::primitives::address!("0x4e59b44847b379578588920cA78FbF26c0B4956C");

/// BIP-32 path for the spending private key. Frozen.
pub const SPEND_PATH: &str = "m/5564'/60'/0'/0'";

/// BIP-32 path for the viewing private key. Frozen with [`SPEND_PATH`].
pub const VIEW_PATH: &str = "m/5564'/60'/0'/1'";

/// Placeholder used in ERC-5564 native-asset announcement metadata.
pub const NATIVE_TOKEN_SENTINEL: Address =
    alloy::primitives::address!("0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE");

sol! {
    /// Canonical ERC-5564 announcer surface (scheme-1 wallets only need `announce` + the event).
    interface IERC5564Announcer {
        function announce(
            uint256 schemeId,
            address stealthAddress,
            bytes ephemeralPubKey,
            bytes metadata
        ) external;

        event Announcement(
            uint256 indexed schemeId,
            address indexed stealthAddress,
            address indexed caller,
            bytes ephemeralPubKey,
            bytes metadata
        );
    }
}

/// Spending + viewing public keys that receivers publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StealthMetaAddress {
    /// Compressed SEC1 spending public key (33 bytes).
    pub spending_pubkey: [u8; 33],
    /// Compressed SEC1 viewing public key (33 bytes).
    pub viewing_pubkey: [u8; 33],
}

/// HD-derived stealth secrets. Signing keys zeroize on drop.
pub struct StealthMetaKeys {
    spend: SigningKey,
    view: SigningKey,
}

/// One-time stealth destination plus the announcement payload the sender must emit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StealthAnnouncement {
    /// One-time secp256k1 address only the recipient can spend.
    pub stealth_address: Address,
    /// Compressed ephemeral public key (33 bytes).
    pub ephemeral_pubkey: [u8; 33],
    /// First byte of `keccak256(compressed_shared_point)`.
    pub view_tag: u8,
}

impl StealthMetaAddress {
    /// True if `s` looks like a stealth meta-address (`st:…` or 66-byte hex keys).
    pub fn looks_like_uri(s: &str) -> bool {
        let s = s.trim();
        s.starts_with("st:") || (s.starts_with("0x") && s.len() == 134)
    }

    /// Parse `st:<chain>:0x<spend><view>` or a bare `0x` + 132 hex chars.
    pub fn parse(input: &str) -> Result<Self, WalletError> {
        let hex_part = if let Some(rest) = input.strip_prefix("st:") {
            let mut parts = rest.splitn(2, ':');
            let _chain = parts
                .next()
                .ok_or_else(|| WalletError::InvalidStealth("missing chain in st: URI".into()))?;
            parts.next().ok_or_else(|| {
                WalletError::InvalidStealth("missing key material in st: URI".into())
            })?
        } else {
            input.trim()
        };
        let hex_part = hex_part.strip_prefix("0x").unwrap_or(hex_part);
        if hex_part.len() != 132 {
            return Err(WalletError::InvalidStealth(format!(
                "expected 132 hex chars of compressed spend||view keys, got {}",
                hex_part.len()
            )));
        }
        let bytes = hex::decode(hex_part)
            .map_err(|e| WalletError::InvalidStealth(format!("invalid hex: {e}")))?;
        let mut spending_pubkey = [0u8; 33];
        let mut viewing_pubkey = [0u8; 33];
        spending_pubkey.copy_from_slice(&bytes[..33]);
        viewing_pubkey.copy_from_slice(&bytes[33..]);
        validate_compressed_pubkey(&spending_pubkey)?;
        validate_compressed_pubkey(&viewing_pubkey)?;
        Ok(Self {
            spending_pubkey,
            viewing_pubkey,
        })
    }

    /// Encode as `st:<chain>:0x<spend_hex><view_hex>` (lowercase hex).
    pub fn to_uri(&self, chain_short: &str) -> String {
        format!(
            "st:{chain_short}:0x{}{}",
            hex::encode(self.spending_pubkey),
            hex::encode(self.viewing_pubkey)
        )
    }
}

impl StealthMetaKeys {
    /// Derive spend/view keys from the vault mnemonic using the frozen paths.
    pub fn from_mnemonic(mnemonic: &Mnemonic) -> Result<Self, WalletError> {
        Ok(Self {
            spend: derive_signing_key(mnemonic, SPEND_PATH)?,
            view: derive_signing_key(mnemonic, VIEW_PATH)?,
        })
    }

    /// Public meta-address corresponding to these secrets.
    pub fn meta_address(&self) -> StealthMetaAddress {
        StealthMetaAddress {
            spending_pubkey: compressed_from_sk(&self.spend),
            viewing_pubkey: compressed_from_sk(&self.view),
        }
    }

    /// Viewing private key (scan only; cannot spend).
    pub fn viewing_key(&self) -> &SigningKey {
        &self.view
    }

    /// Spending private key (combined with the announcement to spend).
    pub fn spending_key(&self) -> &SigningKey {
        &self.spend
    }
}

/// Generate a stealth address for `meta`.
///
/// If `ephemeral_private` is `None`, a fresh CSPRNG key is used (production send).
pub fn generate_stealth_address(
    meta: &StealthMetaAddress,
    ephemeral_private: Option<&SigningKey>,
) -> Result<StealthAnnouncement, WalletError> {
    let owned;
    let eph = match ephemeral_private {
        Some(k) => k,
        None => {
            owned = SigningKey::random(&mut rand::rngs::OsRng);
            &owned
        }
    };
    let ephemeral_pubkey = compressed_from_sk(eph);
    let shared = ecdh_compressed(eph, &meta.viewing_pubkey)?;
    let hashed = keccak256(shared);
    let view_tag = hashed[0];
    let tweak = scalar_from_hash(hashed);
    let spend_point = projective_from_compressed(&meta.spending_pubkey)?;
    let stealth_point = spend_point + ProjectivePoint::GENERATOR * tweak;
    if bool::from(stealth_point.is_identity()) {
        return Err(WalletError::InvalidStealth(
            "stealth public key is the identity point".into(),
        ));
    }
    Ok(StealthAnnouncement {
        stealth_address: address_from_point(&stealth_point),
        ephemeral_pubkey,
        view_tag,
    })
}

/// Return whether `announcement` was generated for this viewing key + spending pubkey.
pub fn check_stealth_address(
    viewing_private: &SigningKey,
    spending_pubkey: &[u8; 33],
    announcement: &StealthAnnouncement,
) -> Result<bool, WalletError> {
    let shared = ecdh_compressed(viewing_private, &announcement.ephemeral_pubkey)?;
    let hashed = keccak256(shared);
    if hashed[0] != announcement.view_tag {
        return Ok(false);
    }
    let tweak = scalar_from_hash(hashed);
    let spend_point = projective_from_compressed(spending_pubkey)?;
    let stealth_point = spend_point + ProjectivePoint::GENERATOR * tweak;
    Ok(address_from_point(&stealth_point) == announcement.stealth_address)
}

/// Compute the one-time spending key for an announcement that belongs to `keys`.
pub fn compute_stealth_key(
    keys: &StealthMetaKeys,
    announcement: &StealthAnnouncement,
) -> Result<SigningKey, WalletError> {
    if !check_stealth_address(
        keys.viewing_key(),
        &keys.meta_address().spending_pubkey,
        announcement,
    )? {
        return Err(WalletError::InvalidStealth(
            "announcement is not for this viewing key".into(),
        ));
    }
    let shared = ecdh_compressed(keys.viewing_key(), &announcement.ephemeral_pubkey)?;
    let hashed = keccak256(shared);
    let tweak = scalar_from_hash(hashed);
    let spend = **keys.spending_key().as_nonzero_scalar();
    let stealth_scalar = spend + tweak;
    let nz = k256::NonZeroScalar::new(stealth_scalar)
        .into_option()
        .ok_or_else(|| WalletError::InvalidStealth("stealth private key is zero".into()))?;
    Ok(SigningKey::from(nz))
}

/// Alloy local signer for the one-time stealth key.
pub fn stealth_signer(key: SigningKey) -> PrivateKeySigner {
    PrivateKeySigner::from_signing_key(key)
}

/// ERC-5564 recommended metadata for a native-asset send (view tag + amount).
pub fn native_announce_metadata(view_tag: u8, amount_wei: U256) -> Vec<u8> {
    let mut meta = Vec::with_capacity(57);
    meta.push(view_tag);
    meta.extend_from_slice(&[0xee, 0xee, 0xee, 0xee]);
    meta.extend_from_slice(NATIVE_TOKEN_SENTINEL.as_slice());
    meta.extend_from_slice(&amount_wei.to_be_bytes::<32>());
    meta
}

/// `Announcement` event topic0 (for `eth_getLogs` filters).
pub fn announcement_topic0() -> B256 {
    IERC5564Announcer::Announcement::SIGNATURE_HASH
}

/// Calldata for `announce(schemeId=1, stealth, ephemeralPubKey, metadata)`.
pub fn encode_announce_calldata(announcement: &StealthAnnouncement, metadata: &[u8]) -> Bytes {
    Bytes::from(
        IERC5564Announcer::announceCall {
            schemeId: U256::from(SCHEME_ID),
            stealthAddress: announcement.stealth_address,
            ephemeralPubKey: Bytes::from(announcement.ephemeral_pubkey.to_vec()),
            metadata: Bytes::from(metadata.to_vec()),
        }
        .abi_encode(),
    )
}

/// Decode a stealth announcement from an announcer log (view tag = metadata[0]).
pub fn stealth_announcement_from_log(
    log: &alloy::rpc::types::Log,
) -> Result<StealthAnnouncement, WalletError> {
    if log.address() != ERC5564_ANNOUNCER {
        return Err(WalletError::InvalidStealth(
            "log is not from the ERC-5564 announcer".into(),
        ));
    }
    let decoded = IERC5564Announcer::Announcement::decode_log(&log.inner)
        .map_err(|e| WalletError::InvalidStealth(format!("invalid Announcement log: {e}")))?;
    let eph = decoded.data.ephemeralPubKey.as_ref();
    if eph.len() != 33 {
        return Err(WalletError::InvalidStealth(format!(
            "ephemeral pubkey must be 33 compressed bytes, got {}",
            eph.len()
        )));
    }
    let metadata = decoded.data.metadata.as_ref();
    if metadata.is_empty() {
        return Err(WalletError::InvalidStealth(
            "announcement metadata missing view tag".into(),
        ));
    }
    let mut ephemeral_pubkey = [0u8; 33];
    ephemeral_pubkey.copy_from_slice(eph);
    Ok(StealthAnnouncement {
        stealth_address: decoded.stealthAddress,
        ephemeral_pubkey,
        view_tag: metadata[0],
    })
}

fn derive_signing_key(mnemonic: &Mnemonic, path: &str) -> Result<SigningKey, WalletError> {
    let seed = mnemonic.to_seed("");
    let derivation = DerivationPath::from_str(path)
        .map_err(|e| WalletError::KeyDerivationFailed(e.to_string()))?;
    let xprv = XPrv::derive_from_path(seed, &derivation)
        .map_err(|e| WalletError::KeyDerivationFailed(e.to_string()))?;
    Ok(xprv.private_key().clone())
}

fn compressed_from_sk(sk: &SigningKey) -> [u8; 33] {
    let pk = PublicKey::from(sk.verifying_key());
    let encoded = pk.to_encoded_point(true);
    encoded
        .as_bytes()
        .try_into()
        .expect("compressed secp256k1 public key is 33 bytes")
}

fn validate_compressed_pubkey(bytes: &[u8; 33]) -> Result<(), WalletError> {
    if bytes[0] != 0x02 && bytes[0] != 0x03 {
        return Err(WalletError::InvalidStealth(
            "compressed public key must start with 0x02 or 0x03".into(),
        ));
    }
    projective_from_compressed(bytes).map(|_| ())
}

fn projective_from_compressed(bytes: &[u8; 33]) -> Result<ProjectivePoint, WalletError> {
    let encoded = EncodedPoint::from_bytes(bytes)
        .map_err(|_| WalletError::InvalidStealth("invalid SEC1 public key".into()))?;
    let affine = AffinePoint::from_encoded_point(&encoded);
    if affine.is_none().into() {
        return Err(WalletError::InvalidStealth(
            "public key is not on secp256k1".into(),
        ));
    }
    Ok(ProjectivePoint::from(affine.unwrap()))
}

fn ecdh_compressed(sk: &SigningKey, pk_compressed: &[u8; 33]) -> Result<[u8; 33], WalletError> {
    let other = projective_from_compressed(pk_compressed)?;
    let shared = other * **sk.as_nonzero_scalar();
    if bool::from(shared.is_identity()) {
        return Err(WalletError::InvalidStealth(
            "ECDH shared secret is the identity point".into(),
        ));
    }
    let encoded = shared.to_affine().to_encoded_point(true);
    encoded
        .as_bytes()
        .try_into()
        .map_err(|_| WalletError::InvalidStealth("compressed shared point encoding failed".into()))
}

fn scalar_from_hash(hash: alloy::primitives::B256) -> Scalar {
    Scalar::reduce(FieldU256::from_be_slice(hash.as_slice()))
}

fn address_from_point(point: &ProjectivePoint) -> Address {
    let uncompressed = point.to_affine().to_encoded_point(false);
    let hash = keccak256(&uncompressed.as_bytes()[1..]);
    Address::from_slice(&hash[12..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ScopeLift SDK sample meta-address (scheme 1 compressed spend||view).
    const SCOPELIFT_META: &str = "st:eth:0x033404e82cd2a92321d51e13064ec13a0fb0192a9fdaaca1cfb47b37bd27ec13970390ad5eca026c05ab5cf4d620a2ac65241b11df004ddca360e954db1b26e3846e";

    /// ScopeLift `getViewTag` fixture: first byte of this keccak is `0x15`.
    const SCOPELIFT_HASHED_SECRET: &str =
        "158ce29a3dd0c8dca524e5776c2ba6361c280e013f87eee5eb799a713a939501";

    const ABANDON: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    const EPH_HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    fn sk_from_hex(h: &str) -> SigningKey {
        let mut raw = hex::decode(h.trim_start_matches("0x")).unwrap();
        while raw.len() < 32 {
            raw.insert(0, 0);
        }
        SigningKey::from_slice(&raw).unwrap()
    }

    #[test]
    fn scopelift_view_tag_is_first_byte_of_keccak() {
        let bytes = hex::decode(SCOPELIFT_HASHED_SECRET).unwrap();
        assert_eq!(bytes[0], 0x15);
    }

    #[test]
    fn parses_scopelift_meta_address() {
        let meta = StealthMetaAddress::parse(SCOPELIFT_META).unwrap();
        assert_eq!(
            hex::encode(meta.spending_pubkey),
            "033404e82cd2a92321d51e13064ec13a0fb0192a9fdaaca1cfb47b37bd27ec1397"
        );
        assert_eq!(
            hex::encode(meta.viewing_pubkey),
            "0390ad5eca026c05ab5cf4d620a2ac65241b11df004ddca360e954db1b26e3846e"
        );
        assert_eq!(meta.to_uri("eth"), SCOPELIFT_META);
    }

    #[test]
    fn generate_check_compute_round_trip() {
        let mnemonic = Mnemonic::parse(ABANDON).unwrap();
        let keys = StealthMetaKeys::from_mnemonic(&mnemonic).unwrap();
        let meta = keys.meta_address();
        let eph = sk_from_hex("2222222222222222222222222222222222222222222222222222222222222222");
        let announcement = generate_stealth_address(&meta, Some(&eph)).unwrap();
        assert!(
            check_stealth_address(keys.viewing_key(), &meta.spending_pubkey, &announcement)
                .unwrap()
        );
        let stealth_sk = compute_stealth_key(&keys, &announcement).unwrap();
        let signer = stealth_signer(stealth_sk);
        assert_eq!(signer.address(), announcement.stealth_address);
    }

    #[test]
    fn view_tag_mismatch_is_not_ours() {
        let mnemonic = Mnemonic::parse(ABANDON).unwrap();
        let keys = StealthMetaKeys::from_mnemonic(&mnemonic).unwrap();
        let mut announcement = generate_stealth_address(
            &keys.meta_address(),
            Some(&sk_from_hex(
                "2222222222222222222222222222222222222222222222222222222222222222",
            )),
        )
        .unwrap();
        announcement.view_tag ^= 0xff;
        assert!(!check_stealth_address(
            keys.viewing_key(),
            &keys.meta_address().spending_pubkey,
            &announcement
        )
        .unwrap());
    }

    #[test]
    fn hd_paths_are_frozen_for_abandon_mnemonic() {
        let mnemonic = Mnemonic::parse(ABANDON).unwrap();
        let keys = StealthMetaKeys::from_mnemonic(&mnemonic).unwrap();
        let meta = keys.meta_address();
        assert_eq!(
            hex::encode(meta.spending_pubkey),
            "027346ef4cc9362fe4c90ba060cc341eab788046139db0626e1b17908aed6c6441"
        );
        assert_eq!(
            hex::encode(meta.viewing_pubkey),
            "02fc10565657ef01035e3197e43fdcbdc4017c8556cb8d43bcae7f68aa79f0d1b4"
        );
    }

    #[test]
    fn native_metadata_starts_with_view_tag() {
        let meta = native_announce_metadata(0xab, U256::from(1u64));
        assert_eq!(meta[0], 0xab);
        assert_eq!(&meta[1..5], &[0xee, 0xee, 0xee, 0xee]);
        assert_eq!(&meta[5..25], NATIVE_TOKEN_SENTINEL.as_slice());
        assert_eq!(meta.len(), 57);
    }

    #[test]
    fn encode_announce_calldata_uses_announce_selector() {
        let mnemonic = Mnemonic::parse(ABANDON).unwrap();
        let keys = StealthMetaKeys::from_mnemonic(&mnemonic).unwrap();
        let announcement = generate_stealth_address(&keys.meta_address(), None).unwrap();
        let data = encode_announce_calldata(
            &announcement,
            &native_announce_metadata(announcement.view_tag, U256::from(1u64)),
        );
        assert_eq!(&data[..4], &IERC5564Announcer::announceCall::SELECTOR);
    }

    #[test]
    fn scopelift_meta_plus_fixed_ephemeral_is_stable() {
        let meta = StealthMetaAddress::parse(SCOPELIFT_META).unwrap();
        let eph = sk_from_hex(EPH_HEX);
        let out = generate_stealth_address(&meta, Some(&eph)).unwrap();
        assert_eq!(out.view_tag, 0xe1);
        assert_eq!(
            hex::encode(out.ephemeral_pubkey),
            "034f355bdcb7cc0af728ef3cceb9615d90684bb5b2ca5f859ab0f0b704075871aa"
        );
        assert_eq!(
            format!("{:#x}", out.stealth_address),
            "0x42245f67fc615dfd10d59fe6aa2e7d7d75ab4fe8"
        );
    }
}
