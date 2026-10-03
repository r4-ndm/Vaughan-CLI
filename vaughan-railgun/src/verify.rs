//! Check who a `shield` call pays, for the approval card.
//!
//! A shield note's recipient is `npk = poseidon(masterPublicKey, random)`,
//! with `random` encrypted to the recipient's viewing key. Decrypting alone
//! isn't proof: calldata can encrypt a random to us while setting an `npk`
//! built from someone else's master key. So each note is decrypted with our
//! viewing key and its `npk` recomputed from our master public key.

use alloy::primitives::U256;
use alloy::sol_types::SolCall;
use broadcaster_core::contracts::railgun::{shieldCall, ShieldRequest};
use broadcaster_core::crypto::shared_key::shared_symmetric_key;
use broadcaster_core::notes::{decrypt_shield_random, Note};
use zeroize::Zeroize;

use crate::keys::RailgunAccount;

/// How many notes in a `shield` call pay this RAILGUN account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShieldRecipients {
    /// Notes whose `npk` was recomputed from our keys.
    pub ours: usize,
    /// All notes in the call.
    pub total: usize,
}

impl ShieldRecipients {
    /// True when every note pays this account.
    #[must_use]
    pub fn all_ours(&self) -> bool {
        self.total > 0 && self.ours == self.total
    }

    /// Value for the card's "Recipient" row.
    #[must_use]
    pub fn card_value(&self) -> String {
        if self.all_ours() {
            "your RAILGUN wallet (verified)".into()
        } else if self.ours == 0 {
            "another RAILGUN wallet (not yours)".into()
        } else {
            format!(
                "{} of {} notes to your RAILGUN wallet (verified); the rest to others",
                self.ours, self.total
            )
        }
    }

    /// Card warning when any note goes to another wallet.
    #[must_use]
    pub fn warning(&self) -> Option<String> {
        (!self.all_ours()).then(|| {
            "Shielded funds go to a RAILGUN wallet that isn't yours — only sign if you meant to pay someone else"
                .into()
        })
    }
}

/// True if `calldata` calls `shield` (selector only; cheap, no keys needed).
#[must_use]
pub fn is_shield_call(calldata: &[u8]) -> bool {
    calldata.starts_with(&shieldCall::SELECTOR)
}

/// Which notes in `shield` calldata pay `account`; `None` if not a shield call.
#[must_use]
pub fn shield_recipients(account: &RailgunAccount, calldata: &[u8]) -> Option<ShieldRecipients> {
    let call = shieldCall::abi_decode(calldata).ok()?;
    let ours = call
        ._shieldRequests
        .iter()
        .filter(|r| pays(account, r))
        .count();
    Some(ShieldRecipients {
        ours,
        total: call._shieldRequests.len(),
    })
}

fn pays(account: &RailgunAccount, request: &ShieldRequest) -> bool {
    let viewing = account.viewing();
    let Ok(mut shared) =
        shared_symmetric_key(viewing.private_key(), &request.ciphertext.shieldKey.0)
    else {
        return false;
    };
    let decrypted = decrypt_shield_random(&request.ciphertext.encryptedBundle, &shared);
    shared.zeroize();
    let Ok(mut random) = decrypted else {
        return false;
    };
    let npk = Note::npk_for(viewing.scan_keys().master_public_key, random);
    random.zeroize();
    npk == U256::from_be_bytes(request.preimage.npk.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{aliases::U120, FixedBytes};
    use broadcaster_core::contracts::railgun::TokenData;
    use broadcaster_core::contracts::shield::build_shield_request;

    use crate::scan::tests::{account, TOKEN};

    fn request_to(to: &RailgunAccount) -> ShieldRequest {
        let keys = to.viewing().scan_keys();
        build_shield_request(
            keys.master_public_key,
            &keys.viewing_public_key,
            TokenData::erc20(TOKEN),
            U120::from(1_000u64),
            &[7u8; 32],
        )
        .unwrap()
    }

    fn calldata(requests: Vec<ShieldRequest>) -> Vec<u8> {
        shieldCall {
            _shieldRequests: requests,
        }
        .abi_encode()
    }

    #[test]
    fn shield_to_self_is_verified() {
        let me = account(0);
        let r = shield_recipients(&me, &calldata(vec![request_to(&me)])).unwrap();
        assert_eq!(r, ShieldRecipients { ours: 1, total: 1 });
        assert_eq!(r.card_value(), "your RAILGUN wallet (verified)");
        assert!(r.warning().is_none());
    }

    #[test]
    fn shield_to_another_wallet_is_flagged() {
        let me = account(0);
        let r = shield_recipients(&me, &calldata(vec![request_to(&account(1))])).unwrap();
        assert_eq!(r, ShieldRecipients { ours: 0, total: 1 });
        assert!(r.card_value().contains("not yours"));
        assert!(r.warning().is_some());
    }

    /// Random encrypted to us, but `npk` built from another master key.
    #[test]
    fn decryptable_note_with_foreign_npk_is_not_ours() {
        let me = account(0);
        let mut forged = request_to(&me);
        forged.preimage.npk = request_to(&account(1)).preimage.npk;
        let r = shield_recipients(&me, &calldata(vec![forged])).unwrap();
        assert_eq!(r.ours, 0);
    }

    #[test]
    fn garbage_ciphertext_is_not_ours() {
        let me = account(0);
        let mut bad = request_to(&me);
        bad.ciphertext.shieldKey = FixedBytes::repeat_byte(0xff);
        let r = shield_recipients(&me, &calldata(vec![bad])).unwrap();
        assert_eq!(r.ours, 0);
    }

    #[test]
    fn mixed_batch_counts_each_note() {
        let me = account(0);
        let data = calldata(vec![request_to(&me), request_to(&account(1))]);
        let r = shield_recipients(&me, &data).unwrap();
        assert_eq!(r, ShieldRecipients { ours: 1, total: 2 });
        assert!(r.card_value().starts_with("1 of 2"));
        assert!(r.warning().is_some());
    }

    #[test]
    fn non_shield_calldata_is_ignored() {
        assert!(!is_shield_call(&[0xde, 0xad, 0xbe, 0xef]));
        assert!(shield_recipients(&account(0), &[0xde, 0xad, 0xbe, 0xef]).is_none());
        assert!(is_shield_call(&calldata(vec![request_to(&account(0))])));
    }
}
