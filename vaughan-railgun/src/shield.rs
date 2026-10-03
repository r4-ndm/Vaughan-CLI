//! Build an ERC-20 shield (public tokens into this wallet's private balance).
//!
//! Follows the official engine: the shield key is `keccak256` of the wallet's
//! EIP-191 signature over `"RAILGUN_SHIELD"` (railgun-rust's
//! `derive_shield_private_key`), so Railway and Vaughan derive the same key.
//! The signature comes from the wallet's normal signing path after the user
//! approves it; this module never sees the EVM private key.
//!
//! Calldata comes from railgun-rust (`build_shield_calldata`). The contract
//! takes its shield fee out of the shielded amount; the plan shows both.
//! Sepolia only until Gate 3 ([`deployment::shield_enabled`]).

use alloy::primitives::{keccak256, Address, U256};
use alloy::providers::Provider;
use alloy::sol;
use broadcaster_core::contracts::shield::{build_approve_calldata, build_shield_calldata};
use vaughan_core::error::WalletError;
use zeroize::Zeroizing;

use crate::deployment::{self, RailgunDeployment};
use crate::keys::RailgunAccount;
use crate::verify::shield_recipients;

/// Message the wallet signs (EIP-191) to derive the shield key.
pub const SHIELD_MESSAGE: &str = "RAILGUN_SHIELD";

/// Fee denominator (`BASIS_POINTS` in `RailgunLogic`).
const BASIS_POINTS: u64 = 10_000;

sol! {
    #[sol(rpc)]
    interface IRailgunFees {
        function shieldFee() external view returns (uint120);
    }
}

/// Shield private key. No `Debug`; wiped on drop.
pub struct ShieldKey(Zeroizing<[u8; 32]>);

impl ShieldKey {
    /// Key from the wallet's `0x` hex EIP-191 signature over [`SHIELD_MESSAGE`]
    /// (65 bytes, `r ‖ s ‖ v` with `v` 27 or 28, as railgun-rust hashes it).
    pub fn from_signature_hex(signature: &str) -> Result<Self, WalletError> {
        let invalid = || WalletError::SigningFailed("unexpected RAILGUN_SHIELD signature".into());
        let bytes = Zeroizing::new(
            hex::decode(signature.trim().trim_start_matches("0x")).map_err(|_| invalid())?,
        );
        if bytes.len() != 65 || !matches!(bytes[64], 27 | 28) {
            return Err(invalid());
        }
        Ok(Self(Zeroizing::new(keccak256(bytes.as_slice()).0)))
    }
}

/// RAILGUN's current shield fee in basis points, read from the contract.
pub async fn shield_fee_bps<P: Provider>(
    provider: &P,
    deployment: &RailgunDeployment,
) -> Result<u16, WalletError> {
    let fee = IRailgunFees::new(deployment.contract, provider)
        .shieldFee()
        .call()
        .await
        .map_err(|e| WalletError::RpcError(format!("RAILGUN shield fee: {e}")))?;
    u16::try_from(fee.to::<u128>())
        .ok()
        .filter(|bps| u64::from(*bps) < BASIS_POINTS)
        .ok_or_else(|| WalletError::Other(format!("RAILGUN shield fee out of range: {fee}")))
}

/// Fee and note value for shielding `amount` (`RailgunLogic.getFee`, inclusive).
#[must_use]
pub fn fee_split(amount: U256, fee_bps: u16) -> (U256, U256) {
    let fee = amount * U256::from(fee_bps) / U256::from(BASIS_POINTS);
    (fee, amount - fee)
}

/// Everything the approval cards and transactions need for one shield.
#[derive(Debug, Clone)]
pub struct ShieldPlan {
    /// ERC-20 being shielded.
    pub token: Address,
    /// RAILGUN contract: the `approve` spender and the shield `to`.
    pub contract: Address,
    /// Amount taken from the public wallet (smallest unit).
    pub amount: U256,
    /// Shield fee in basis points.
    pub fee_bps: u16,
    /// Part of `amount` RAILGUN keeps as its fee.
    pub fee: U256,
    /// Part of `amount` that lands in the private balance.
    pub net: U256,
    /// `approve(contract, amount)`: exact amount, never unlimited.
    pub approve_calldata: Vec<u8>,
    /// `shield([request])` paying this wallet's 0zk address.
    pub shield_calldata: Vec<u8>,
}

/// Plan shielding `amount` of `token` into `account`'s private balance.
///
/// The built calldata is checked with [`shield_recipients`] before it's
/// returned, so a plan always pays this wallet.
pub fn plan(
    deployment: &RailgunDeployment,
    account: &RailgunAccount,
    key: &ShieldKey,
    token: Address,
    amount: U256,
    fee_bps: u16,
) -> Result<ShieldPlan, WalletError> {
    if !deployment::shield_enabled(deployment.chain_id) {
        return Err(WalletError::Other(
            "Shielding is Sepolia-only for now".into(),
        ));
    }
    if token == Address::ZERO {
        return Err(WalletError::InvalidTransaction("token address".into()));
    }
    if amount.is_zero() || amount.bit_len() > 120 {
        return Err(WalletError::InvalidAmount(
            "shield amount must be above zero and fit a RAILGUN note".into(),
        ));
    }
    if u64::from(fee_bps) >= BASIS_POINTS {
        return Err(WalletError::Other("RAILGUN shield fee out of range".into()));
    }
    let (fee, net) = fee_split(amount, fee_bps);
    if net.is_zero() {
        return Err(WalletError::InvalidAmount(
            "amount is too small: the shield fee would take all of it".into(),
        ));
    }
    let keys = account.viewing().scan_keys();
    let shield_calldata = build_shield_calldata(
        keys.master_public_key,
        &keys.viewing_public_key,
        token,
        amount,
        &key.0,
    )
    .map_err(|e| WalletError::Other(format!("RAILGUN shield request: {e}")))?;
    if !shield_recipients(account, &shield_calldata).is_some_and(|r| r.all_ours()) {
        return Err(WalletError::Other(
            "built shield doesn't pay this RAILGUN wallet".into(),
        ));
    }
    Ok(ShieldPlan {
        token,
        contract: deployment.contract,
        amount,
        fee_bps,
        fee,
        net,
        approve_calldata: build_approve_calldata(deployment.contract, amount),
        shield_calldata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::SignerSync;
    use alloy::sol_types::SolCall;
    use broadcaster_core::contracts::railgun::approveCall;
    use broadcaster_core::contracts::shield::derive_shield_private_key;

    use crate::deployment::{for_chain, SEPOLIA_CHAIN_ID};
    use crate::scan::tests::{account, TOKEN};

    /// Anvil's public dev key 0 (test-only, never funded on a real network).
    const ANVIL_KEY0: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    fn sepolia() -> RailgunDeployment {
        for_chain(SEPOLIA_CHAIN_ID).unwrap()
    }

    fn test_key() -> ShieldKey {
        ShieldKey(Zeroizing::new([9u8; 32]))
    }

    /// Same key as railgun-rust when the signature comes from Alloy (the
    /// signer behind `WalletState::sign_message`).
    #[test]
    fn shield_key_matches_upstream_derivation() {
        let signer: PrivateKeySigner = ANVIL_KEY0.parse().unwrap();
        let signature = signer.sign_message_sync(SHIELD_MESSAGE.as_bytes()).unwrap();
        let ours =
            ShieldKey::from_signature_hex(&format!("0x{}", hex::encode(signature.as_bytes())))
                .unwrap();
        let mut raw = [0u8; 32];
        hex::decode_to_slice(ANVIL_KEY0, &mut raw).unwrap();
        assert_eq!(*ours.0, derive_shield_private_key(&raw).unwrap());
    }

    #[test]
    fn rejects_malformed_signatures() {
        assert!(ShieldKey::from_signature_hex("0x1234").is_err());
        let mut sig = [0u8; 65];
        sig[64] = 1;
        assert!(ShieldKey::from_signature_hex(&hex::encode(sig)).is_err());
        sig[64] = 27;
        assert!(ShieldKey::from_signature_hex(&hex::encode(sig)).is_ok());
    }

    #[test]
    fn fee_matches_railgun_get_fee() {
        assert_eq!(
            fee_split(U256::from(10_000u64), 25),
            (U256::from(25u64), U256::from(9_975u64))
        );
        assert_eq!(
            fee_split(U256::from(399u64), 25),
            (U256::ZERO, U256::from(399u64))
        );
        assert_eq!(
            fee_split(U256::from(1_000u64), 0),
            (U256::ZERO, U256::from(1_000u64))
        );
    }

    #[test]
    fn plan_pays_this_wallet_with_exact_approval() {
        let me = account(0);
        let amount = U256::from(1_000_000u64);
        let p = plan(&sepolia(), &me, &test_key(), TOKEN, amount, 25).unwrap();
        assert_eq!(p.contract, sepolia().contract);
        assert_eq!(
            (p.fee, p.net),
            (U256::from(2_500u64), U256::from(997_500u64))
        );
        assert!(shield_recipients(&me, &p.shield_calldata)
            .unwrap()
            .all_ours());
        assert!(shield_recipients(&account(1), &p.shield_calldata)
            .unwrap()
            .warning()
            .is_some());
        let approve = approveCall::abi_decode(&p.approve_calldata).unwrap();
        assert_eq!((approve.spender, approve.amount), (p.contract, amount));
    }

    #[test]
    fn plan_rejects_bad_input() {
        let me = account(0);
        let k = test_key();
        let one = U256::from(1u64);
        assert!(plan(&sepolia(), &me, &k, Address::ZERO, one, 25).is_err());
        assert!(plan(&sepolia(), &me, &k, TOKEN, U256::ZERO, 25).is_err());
        assert!(plan(&sepolia(), &me, &k, TOKEN, U256::ONE << 120, 25).is_err());
        assert!(plan(&sepolia(), &me, &k, TOKEN, one, 10_000).is_err());
        assert!(plan(&sepolia(), &me, &k, TOKEN, one, 9_999).is_ok());
        let mainnet = for_chain(1).unwrap();
        assert!(plan(&mainnet, &me, &k, TOKEN, one, 25).is_err());
    }

    /// Live: the Sepolia proxy answers `shieldFee()` (25 bp on 2026-10-03).
    #[tokio::test]
    #[ignore = "network: Sepolia RPC"]
    async fn live_sepolia_shield_fee() {
        let url = std::env::var("RAILGUN_SEPOLIA_RPC")
            .unwrap_or_else(|_| "https://sepolia.gateway.tenderly.co".into());
        let provider = crate::scan::connect(&url).unwrap();
        let bps = shield_fee_bps(&provider, &sepolia()).await.unwrap();
        assert!(bps < 10_000);
    }
}
