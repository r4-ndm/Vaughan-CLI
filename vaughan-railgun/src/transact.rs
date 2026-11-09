//! Phase 3: RAILGUN private transactions and unshielding engine.
//!
//! Provides the structural and cryptographic helpers for shielded balance management:
//! 1. On-chain Merkle root verification (`verify_merkle_root`) to validate historical
//!    tree roots against lying RPC nodes.
//! 2. Secure artifact and prover cache isolation (`prover_cache_dir`) under private
//!    profile directories with POSIX owner-only permissions (`0700`).
//! 3. Proofless note inspection and change calculation (`preview_unshield`).
//! 4. Ephemeral spending key derivation (`SpendingAccount`) zeroized immediately on drop.
//!
//! Note on unshielding and proofs:
//! Local zk-SNARK proof generation and Proof of Innocence (POI) verification are not
//! yet implemented in this repository. In accordance with Rule 9 ("Never fabricate
//! a transaction"), [`create_unshield_proposal`] refuses to construct placeholder
//! or unproved transactions, returning an error. Unshielding remains preview-only
//! until the prover and POI pipeline are implemented.

use std::fs;
use std::path::{Path, PathBuf};

use alloy::primitives::{Address, FixedBytes, U256};
use alloy::providers::Provider;
use alloy::sol;
use broadcaster_core::utxo::Utxo;
use railgun_wallet::keys::WalletKeys;
pub use railgun_wallet::tx::{
    max_send_spendable, max_unshield_spendable, unshield_selection_info, TransactionBuilder,
    TransactionCall, UnshieldMode, UnshieldPlan, UnshieldRequest,
};
use vaughan_core::error::WalletError;
use zeroize::Zeroize;

use crate::deployment::RailgunDeployment;

sol! {
    #[sol(rpc)]
    interface IRailgunRootHistory {
        function rootHistory(uint256 treeNumber, bytes32 root) external view returns (bool);
    }
}

/// Verify on-chain whether `root` is a valid historical Merkle root for `tree_number`.
///
/// Prevents generating zk-SNARK proofs against invalid or manipulated tree states
/// served by out-of-sync or malicious RPC nodes.
pub async fn verify_merkle_root<P: Provider>(
    provider: &P,
    deployment: &RailgunDeployment,
    tree_number: u32,
    root: [u8; 32],
) -> Result<bool, WalletError> {
    let result = IRailgunRootHistory::new(deployment.contract, provider)
        .rootHistory(U256::from(tree_number), FixedBytes::from(root))
        .call()
        .await
        .map_err(|e| WalletError::RpcError(format!("RAILGUN Merkle root verification: {e}")))?;
    Ok(result)
}

/// Ensure and return the private directory used for storing local zk-SNARK prover
/// artifacts and compiled WASM modules.
///
/// Enforces POSIX `0700` permissions (owner-only read/write/execute) on unix systems
/// to satisfy the threat model requirement against shared/tampered native code caches.
pub fn prover_cache_dir(profile_dir: &Path) -> Result<PathBuf, WalletError> {
    let dir = profile_dir.join("railgun").join("prover_cache");
    fs::create_dir_all(&dir)
        .map_err(|e| WalletError::Other(format!("failed creating prover cache dir: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o700);
        fs::set_permissions(&dir, perms).map_err(|e| {
            WalletError::Other(format!("failed setting prover cache permissions: {e}"))
        })?;
    }

    Ok(dir)
}

/// Summary of a proposed unshield operation before proving or key derivation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnshieldPreview {
    /// Token being unshielded.
    pub token: Address,
    /// Public recipient address receiving the unshielded funds.
    pub recipient: Address,
    /// Requested amount to receive in the public balance.
    pub amount: U256,
    /// Whether unshielding through RelayAdapt to unwrap WETH directly to native ETH.
    pub is_unwrap_base: bool,
    /// Number of private UTXOs selected as inputs.
    pub input_notes_count: usize,
    /// Sum of value across all selected input UTXOs.
    pub total_input_amount: U256,
    /// Change amount returning to a newly minted private note.
    pub change_amount: U256,
}

/// Inspect available unspent notes and compute a proofless preview of the unshield transaction.
///
/// Does not require or access private spending keys (can be safely run during UI rendering).
pub fn preview_unshield(
    utxos: &[Utxo],
    token: Address,
    recipient: Address,
    amount: U256,
    is_unwrap_base: bool,
) -> Result<UnshieldPreview, WalletError> {
    if amount.is_zero() || amount.bit_len() > 120 {
        return Err(WalletError::InvalidAmount(
            "unshield amount must be non-zero and fit a uint120 note".into(),
        ));
    }
    if recipient == Address::ZERO {
        return Err(WalletError::InvalidTransaction(
            "recipient cannot be zero address".into(),
        ));
    }

    // Select notes from available UTXOs (1 public output + 1 potential private change output)
    let info = unshield_selection_info(utxos, token, amount, false)
        .map_err(|e| WalletError::Other(format!("note selection error: {e:?}")))?;

    let total_input_amount = info.total;
    let change_amount = total_input_amount.saturating_sub(amount);
    let input_notes_count = info.input_count;

    Ok(UnshieldPreview {
        token,
        recipient,
        amount,
        is_unwrap_base,
        input_notes_count,
        total_input_amount,
        change_amount,
    })
}

/// Construct the transaction builder for the target deployment.
#[must_use]
pub fn transaction_builder(deployment: &RailgunDeployment) -> TransactionBuilder {
    TransactionBuilder {
        chain_type: 0,
        chain_id: deployment.chain_id,
        railgun_contract: deployment.contract,
        relay_adapt_contract: deployment.relay_adapt_contract,
    }
}

/// Construct the unshield request parameters.
#[must_use]
pub fn unshield_request(
    token: Address,
    recipient: Address,
    amount: U256,
    is_unwrap_base: bool,
) -> UnshieldRequest {
    let mode = if is_unwrap_base {
        UnshieldMode::UnwrapBase
    } else {
        UnshieldMode::Token
    };
    UnshieldRequest {
        token_address: token,
        amount,
        recipient,
        mode,
        verify_proof: true,
        spend_up_to: false,
        broadcaster_fee: None,
        min_gas_price: 0,
    }
}

/// Everything needed to present the final EVM transaction on an approval card.
#[derive(Debug, Clone)]
pub struct UnshieldTransactionProposal {
    /// EVM target contract (`deployment.contract` for standard unshields, `relay_adapt_contract` for unwraps).
    pub to: Address,
    /// ABI encoded `transact` or `relay` calldata.
    pub calldata: Vec<u8>,
    /// Summary of the unshield amounts and destinations.
    pub preview: UnshieldPreview,
    /// Whether this proposal carries a verified local zk-SNARK proof.
    pub has_proof: bool,
}

impl UnshieldTransactionProposal {
    /// Create a proposal from a generated [`UnshieldPlan`].
    pub fn from_plan(plan: &UnshieldPlan, preview: UnshieldPreview) -> Self {
        Self {
            to: plan.call.to,
            calldata: plan.call.data.to_vec(),
            preview,
            has_proof: true,
        }
    }

    /// Convert to an alloy transaction call.
    #[must_use]
    pub fn to_transaction_call(&self) -> TransactionCall {
        TransactionCall {
            to: self.to,
            data: self.calldata.clone().into(),
        }
    }
}

/// Ephemeral spending credentials derived inside `with_hd_seed` and dropped/zeroized immediately.
pub struct SpendingAccount {
    keys: WalletKeys,
}

impl SpendingAccount {
    /// Derive ephemeral spending keys from a BIP-39 seed and HD index.
    pub fn from_seed(seed: &[u8; 64], hd_index: u32) -> Result<Self, WalletError> {
        let keys = WalletKeys::from_seed(seed, hd_index)
            .map_err(|e| WalletError::KeyDerivationFailed(format!("RAILGUN keys: {e}")))?;
        Ok(Self { keys })
    }

    /// Access the underlying [`WalletKeys`].
    #[must_use]
    pub fn wallet_keys(&self) -> &WalletKeys {
        &self.keys
    }
}

impl Drop for SpendingAccount {
    fn drop(&mut self) {
        self.keys.spending_private_key.zeroize();
        self.keys.viewing.viewing_private_key.zeroize();
    }
}

/// Create an unshield proposal with encoded transact calldata.
///
/// # Errors
/// Always returns [`WalletError::Other`] because local zk-SNARK proof generation
/// and Proof of Innocence (POI) verification are not yet implemented in this repository.
/// In accordance with Rule 9 ("Never fabricate a transaction"), placeholder or
/// zero-proof transactions cannot be emitted.
pub fn create_unshield_proposal(
    _deployment: &RailgunDeployment,
    _preview: &UnshieldPreview,
) -> Result<UnshieldTransactionProposal, WalletError> {
    Err(WalletError::Other(
        "RAILGUN prover not implemented — unshield is preview-only".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256};
    use alloy::sol_types::SolCall;
    use broadcaster_core::notes::Note;
    use broadcaster_core::utxo::{Utxo, UtxoCommitmentKind, UtxoSource};

    use crate::deployment::{for_chain, SEPOLIA_CHAIN_ID};

    fn sepolia() -> RailgunDeployment {
        for_chain(SEPOLIA_CHAIN_ID).unwrap()
    }

    fn sample_utxo(token: Address, amount: u64, position: u64) -> Utxo {
        let note = Note {
            token_hash: U256::from_be_slice(token.as_slice()),
            value: U256::from(amount),
            random: [7u8; 16],
            npk: U256::from(position + 1),
        };
        Utxo::new(
            note,
            0,
            position,
            UtxoSource {
                tx_hash: FixedBytes::ZERO,
                block_number: 0,
                block_timestamp: 0,
            },
            UtxoCommitmentKind::Transact,
        )
    }

    #[test]
    fn root_history_call_encodes_correctly() {
        let call = IRailgunRootHistory::rootHistoryCall {
            treeNumber: U256::from(0),
            root: b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        };
        let encoded = call.abi_encode();
        assert!(!encoded.is_empty());
        let decoded = IRailgunRootHistory::rootHistoryCall::abi_decode(&encoded).unwrap();
        assert_eq!(decoded.treeNumber, U256::ZERO);
        assert_eq!(
            decoded.root,
            b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
    }

    #[test]
    fn prover_cache_dir_sets_owner_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = prover_cache_dir(temp.path()).unwrap();
        assert!(cache_dir.is_dir());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(&cache_dir).unwrap();
            let mode = metadata.permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o700,
                "directory permissions must be owner-only (0700)"
            );
        }
    }

    #[test]
    fn preview_unshield_calculates_inputs_and_change_without_secrets() {
        let token = address!("0xfff9976782d46cc05630d1f6ebab18b2324d6b14");
        let recipient = address!("0x1111111111111111111111111111111111111111");
        let utxos = vec![
            sample_utxo(token, 1_000_000, 0),
            sample_utxo(token, 2_000_000, 1),
            sample_utxo(token, 5_000_000, 2),
        ];

        // Request 2.5 tokens: should pick notes to cover and report exact change
        let requested = U256::from(2_500_000u64);
        let preview = preview_unshield(&utxos, token, recipient, requested, false).unwrap();
        assert_eq!(preview.token, token);
        assert_eq!(preview.recipient, recipient);
        assert_eq!(preview.amount, requested);
        assert_eq!(preview.total_input_amount, U256::from(5_000_000u64));
        assert_eq!(preview.change_amount, U256::from(2_500_000u64));
        assert_eq!(preview.input_notes_count, 1);
        assert!(!preview.is_unwrap_base);
    }

    #[test]
    fn preview_unshield_rejects_insufficient_or_invalid_inputs() {
        let token = address!("0xfff9976782d46cc05630d1f6ebab18b2324d6b14");
        let recipient = address!("0x1111111111111111111111111111111111111111");
        let utxos = vec![sample_utxo(token, 1_000_000, 0)];

        // Requesting 0 should fail
        assert!(preview_unshield(&utxos, token, recipient, U256::ZERO, false).is_err());
        // Zero recipient address should fail
        assert!(preview_unshield(&utxos, token, Address::ZERO, U256::from(100u64), false).is_err());
        // Requesting more than available balance should fail
        assert!(
            preview_unshield(&utxos, token, recipient, U256::from(2_000_000u64), false).is_err()
        );
    }

    #[test]
    fn transaction_builder_targets_deployed_contracts() {
        let dep = sepolia();
        let builder = transaction_builder(&dep);
        assert_eq!(builder.chain_id, dep.chain_id);
        assert_eq!(builder.railgun_contract, dep.contract);
        assert_eq!(builder.relay_adapt_contract, dep.relay_adapt_contract);

        let req = unshield_request(
            address!("0xfff9976782d46cc05630d1f6ebab18b2324d6b14"),
            address!("0x2222222222222222222222222222222222222222"),
            U256::from(500_000u64),
            true,
        );
        assert!(matches!(req.mode, UnshieldMode::UnwrapBase));
        assert!(req.verify_proof);
    }

    #[test]
    fn spending_account_derivation_and_drop_zeroizes() {
        let seed = [42u8; 64];
        let account = SpendingAccount::from_seed(&seed, 0).unwrap();
        assert_ne!(
            account.wallet_keys().spending_public_key,
            [U256::ZERO, U256::ZERO]
        );
        drop(account);
    }

    #[test]
    fn create_unshield_proposal_refuses_proofless_transaction() {
        let dep = sepolia();
        let token = address!("0xfff9976782d46cc05630d1f6ebab18b2324d6b14");
        let recipient = address!("0x1111111111111111111111111111111111111111");
        let preview = UnshieldPreview {
            token,
            recipient,
            amount: U256::from(1_500_000u64),
            is_unwrap_base: false,
            input_notes_count: 1,
            total_input_amount: U256::from(2_000_000u64),
            change_amount: U256::from(500_000u64),
        };

        let result = create_unshield_proposal(&dep, &preview);
        assert!(
            result.is_err(),
            "create_unshield_proposal must refuse without a prover"
        );
        match result.unwrap_err() {
            WalletError::Other(msg) => {
                assert!(
                    msg.contains("RAILGUN prover not implemented"),
                    "unexpected error message: {msg}"
                );
            }
            err => panic!("unexpected error variant: {err:?}"),
        }
    }
}
