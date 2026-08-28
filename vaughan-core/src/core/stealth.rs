//! Wallet-level ERC-5564 send / scan / sweep used by the TUI.

use std::str::FromStr;

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;

use crate::chains::evm::adapter::EvmAdapter;
use crate::chains::{ChainAdapter, ChainTransaction, Fee, FeeDetails, TxHash};
use crate::core::transaction::TransactionService;
use crate::core::wallet::WalletState;
use crate::error::WalletError;
use crate::security::stealth::{
    announcement_topic0, check_stealth_address, compute_stealth_key, encode_announce_calldata,
    generate_stealth_address, native_announce_metadata, stealth_announcement_from_log,
    stealth_signer, StealthAnnouncement, StealthMetaAddress, ERC5564_ANNOUNCER,
};

/// First PulseChain testnet v4 block that contains the canonical announcer.
const ANNOUNCER_FROM_BLOCK_943: u64 = 25_174_175;

/// Result of paying a stealth meta-address (native transfer + announce).
#[derive(Debug, Clone)]
pub struct StealthSendResult {
    pub stealth_address: Address,
    pub pay_tx: TxHash,
    pub announce_tx: TxHash,
}

/// A scanned announcement that belongs to this wallet, plus on-chain balance.
#[derive(Debug, Clone)]
pub struct StealthNote {
    pub announcement: StealthAnnouncement,
    pub balance_wei: U256,
    pub balance_formatted: String,
}

impl WalletState {
    /// `st:<short>:0x…` meta-address for the unlocked vault on the active chain.
    pub fn stealth_uri(&self) -> Result<String, WalletError> {
        let keys = self.unlocked_accounts()?.stealth_keys()?;
        let short = self.networks().active().eip3770_short_name();
        Ok(keys.meta_address().to_uri(short))
    }

    /// Prepare a one-time stealth destination from a recipient meta-address URI.
    pub fn prepare_stealth_payment(
        &self,
        recipient_uri: &str,
    ) -> Result<StealthAnnouncement, WalletError> {
        let _ = self.unlocked_accounts()?;
        let meta = StealthMetaAddress::parse(recipient_uri)?;
        generate_stealth_address(&meta, None)
    }

    /// Native transfer to the stealth address plus `announce()` on the canonical
    /// announcer. Fails if the announcer has no code on this chain.
    pub async fn send_stealth(
        &self,
        announcement: &StealthAnnouncement,
        value_wei: &str,
    ) -> Result<StealthSendResult, WalletError> {
        self.require_announcer().await?;
        let pay_tx = self
            .send(&format!("{:#x}", announcement.stealth_address), value_wei)
            .await?;
        let value = U256::from_str(value_wei)
            .map_err(|_| WalletError::InvalidAmount(format!("invalid amount: {value_wei}")))?;
        let metadata = native_announce_metadata(announcement.view_tag, value);
        let data = encode_announce_calldata(announcement, &metadata);
        let from = self.unlocked_accounts()?.active_address();
        let net = self.networks().active();
        let ChainTransaction::Evm(announce_tx) = TransactionService::new().build_contract_call(
            from,
            format!("{ERC5564_ANNOUNCER:#x}"),
            &data.to_string(),
            "0",
            net.chain_id,
        )?
        else {
            return Err(WalletError::InvalidTransaction(
                "expected an EVM transaction".into(),
            ));
        };
        let announce_tx = self.send_transaction(announce_tx).await?;
        Ok(StealthSendResult {
            stealth_address: announcement.stealth_address,
            pay_tx: TxHash(pay_tx.hash),
            announce_tx,
        })
    }

    /// Scan announcer logs for notes owned by this vault that can still be swept.
    ///
    /// Leftover gas dust after a sweep is omitted: a note must cover sweep gas.
    pub async fn scan_stealth_notes(&self) -> Result<Vec<StealthNote>, WalletError> {
        let keys = self.unlocked_accounts()?.stealth_keys()?;
        let adapter = self.read_adapter().await?;
        self.require_announcer_on(&adapter).await?;
        let min_sweep = self.min_sweep_wei(&adapter).await?;
        let latest = adapter
            .with_provider(|provider| async move {
                provider
                    .get_block_number()
                    .await
                    .map_err(|e| WalletError::RpcError(e.to_string()))
            })
            .await?;
        let from = announcer_from_block(self.networks().active().chain_id, latest);
        let filter = Filter::new()
            .address(ERC5564_ANNOUNCER)
            .event_signature(announcement_topic0())
            .from_block(from)
            .to_block(latest);
        let logs = adapter
            .with_provider(|provider| {
                let filter = filter.clone();
                async move {
                    provider
                        .get_logs(&filter)
                        .await
                        .map_err(|e| WalletError::RpcError(e.to_string()))
                }
            })
            .await?;
        let spend_pk = keys.meta_address().spending_pubkey;
        let mut notes = Vec::new();
        for log in logs {
            let Ok(announcement) = stealth_announcement_from_log(&log) else {
                continue;
            };
            if !check_stealth_address(keys.viewing_key(), &spend_pk, &announcement)? {
                continue;
            }
            let addr = announcement.stealth_address;
            let balance = adapter.get_balance(&format!("{addr:#x}")).await?;
            let raw = U256::from_str(&balance.raw).unwrap_or(U256::ZERO);
            if raw <= min_sweep {
                continue;
            }
            notes.push(StealthNote {
                announcement,
                balance_wei: raw,
                balance_formatted: format!("{} {}", balance.formatted, balance.token.symbol),
            });
        }
        Ok(notes)
    }

    /// Sweep a scanned note back to the active public account.
    pub async fn sweep_stealth_note(&self, note: &StealthNote) -> Result<TxHash, WalletError> {
        let keys = self.unlocked_accounts()?.stealth_keys()?;
        let dest = self.unlocked_accounts()?.active_address().to_string();
        let sk = compute_stealth_key(&keys, &note.announcement)?;
        let signer: PrivateKeySigner =
            stealth_signer(sk).with_chain_id(Some(self.networks().active().chain_id));
        let net = self.networks().active();
        let (primary, fallbacks) = self.rpc_endpoints_for(net);
        let adapter =
            EvmAdapter::with_signer(&primary, net.chain_id, &net.name, signer, &fallbacks).await?;
        let from = format!("{:#x}", note.announcement.stealth_address);
        let mut tx = TransactionService::new().build_native_transfer(
            &from,
            &dest,
            note.balance_wei.to_string(),
            net.chain_id,
        )?;
        let fee = adapter.estimate_fee(&tx).await?;
        let gas_cost = stealth_gas_cost_wei(&fee)?;
        if note.balance_wei <= gas_cost {
            return Err(WalletError::InvalidAmount(
                "stealth note is too small to cover sweep gas (sender stipend too low)".into(),
            ));
        }
        let send_wei = note.balance_wei - gas_cost;
        let ChainTransaction::Evm(ref mut evm) = tx else {
            return Err(WalletError::InvalidTransaction(
                "expected an EVM transaction".into(),
            ));
        };
        evm.value = send_wei.to_string();
        TransactionService::new().apply_fee(&mut tx, &fee)?;
        adapter.send_transaction(tx).await
    }

    async fn read_adapter(&self) -> Result<EvmAdapter, WalletError> {
        let net = self.networks().active();
        let (primary, fallbacks) = self.rpc_endpoints_for(net);
        EvmAdapter::new(&primary, net.chain_id, &net.name, &fallbacks).await
    }

    /// Conservative sweep cost (`gas_limit * max_fee`) used to hide leftover dust.
    async fn min_sweep_wei(&self, adapter: &EvmAdapter) -> Result<U256, WalletError> {
        let from = self.unlocked_accounts()?.active_address().to_string();
        let dummy = TransactionService::new().build_native_transfer(
            &from,
            &from,
            "1",
            self.networks().active().chain_id,
        )?;
        let fee = adapter.estimate_fee(&dummy).await?;
        stealth_gas_cost_wei(&fee)
    }

    async fn require_announcer(&self) -> Result<(), WalletError> {
        let adapter = self.read_adapter().await?;
        self.require_announcer_on(&adapter).await
    }

    async fn require_announcer_on(&self, adapter: &EvmAdapter) -> Result<(), WalletError> {
        let code = adapter
            .with_provider(|provider| async move {
                provider
                    .get_code_at(ERC5564_ANNOUNCER)
                    .await
                    .map_err(|e| WalletError::RpcError(e.to_string()))
            })
            .await?;
        if code.is_empty() {
            return Err(WalletError::Other(
                "stealth announcer is not deployed on this network (PulseChain testnet 943 for now)"
                    .into(),
            ));
        }
        Ok(())
    }
}

fn announcer_from_block(chain_id: u64, latest: u64) -> u64 {
    match chain_id {
        // Live 943: skip pre-deploy history. Local anvil reuses chain id 943
        // with a tiny head, so `min(deploy, latest)` would equal `latest` and
        // miss notes once later blocks are mined.
        943 if latest < ANNOUNCER_FROM_BLOCK_943 => 0,
        943 => ANNOUNCER_FROM_BLOCK_943,
        _ => latest.saturating_sub(50_000),
    }
}

fn stealth_gas_cost_wei(fee: &Fee) -> Result<U256, WalletError> {
    match &fee.details {
        FeeDetails::Evm {
            gas_limit,
            max_fee_per_gas,
            ..
        } => {
            let max_fee = max_fee_per_gas
                .as_deref()
                .ok_or_else(|| WalletError::GasEstimationFailed("missing max fee".into()))?;
            let parsed = U256::from_str(max_fee)
                .map_err(|_| WalletError::InvalidAmount(format!("invalid max fee: {max_fee}")))?;
            Ok(U256::from(*gas_limit) * parsed)
        }
        _ => Err(WalletError::GasEstimationFailed(
            "expected EVM fee details".into(),
        )),
    }
}

/// Re-export for TUI matching of recipient fields.
pub fn looks_like_stealth_uri(s: &str) -> bool {
    StealthMetaAddress::looks_like_uri(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcer_from_block_uses_genesis_on_anvil_sized_heads() {
        assert_eq!(announcer_from_block(943, 0), 0);
        assert_eq!(announcer_from_block(943, 12), 0);
        assert_eq!(announcer_from_block(943, 25_174_174), 0);
        assert_eq!(announcer_from_block(943, 25_174_175), 25_174_175);
        assert_eq!(announcer_from_block(943, 30_000_000), 25_174_175);
    }
}
