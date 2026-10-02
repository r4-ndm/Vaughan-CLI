//! Find this wallet's shielded notes by scanning RAILGUN contract logs.
//!
//! Fetches the RAILGUN events over plain `eth_getLogs` (chunked, resumable
//! from a checkpoint), decrypts them locally with railgun-rust, and marks a
//! note spent when its nullifier shows up. The RPC sees which block ranges are
//! requested, never which notes are ours.
//!
//! Trust: the result reflects what the RPC returned. An RPC can't forge notes
//! to us, but it could hide some; that's acceptable for a read-only display.
//! Spends (Phase 3) must verify Merkle roots on-chain first.

use std::collections::{BTreeSet, HashMap};
use std::ops::ControlFlow;

use alloy::primitives::{B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::client::ClientBuilder;
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::SolEvent;
use alloy::transports::layers::RetryBackoffLayer;
use broadcaster_core::contracts::railgun::{
    CommitmentBatch, GeneratedCommitmentBatch, Nullified, Nullifiers, RailgunLegacyShieldEvents,
    Shield, Transact,
};
use broadcaster_core::deployment::RailgunDeployment;
use broadcaster_core::utxo::WalletUtxo;
use railgun_wallet::scan::{parse_wallet_delta_from_logs, WalletLogDelta};
use vaughan_core::error::WalletError;

use crate::keys::RailgunAccount;

/// Every event railgun-rust's log parser understands.
fn event_topics() -> Vec<B256> {
    vec![
        Transact::SIGNATURE_HASH,
        Shield::SIGNATURE_HASH,
        RailgunLegacyShieldEvents::Shield::SIGNATURE_HASH,
        Nullifiers::SIGNATURE_HASH,
        Nullified::SIGNATURE_HASH,
        CommitmentBatch::SIGNATURE_HASH,
        GeneratedCommitmentBatch::SIGNATURE_HASH,
    ]
}

/// HTTP provider for scanning, with backoff on rate-limit replies (a full
/// scan makes hundreds of `eth_getLogs` calls; free RPCs throttle them).
pub fn connect(rpc_url: &str) -> Result<impl Provider + Clone + use<>, WalletError> {
    let url = rpc_url
        .parse()
        .map_err(|e| WalletError::NetworkError(format!("invalid RPC URL: {e}")))?;
    let client = ClientBuilder::default()
        .layer(RetryBackoffLayer::new(10, 1_000, 330))
        .http(url);
    Ok(ProviderBuilder::new().connect_client(client))
}

/// Scanner tuning.
#[derive(Debug, Clone, Copy)]
pub struct ScanOptions {
    /// Blocks per `eth_getLogs` call; halved on RPC errors down to `min_chunk`.
    pub chunk: u64,
    /// Smallest chunk before an RPC error is reported.
    pub min_chunk: u64,
    /// Stay this many blocks behind the head so shallow reorgs don't stick.
    pub confirmations: u64,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            chunk: 5_000,
            min_chunk: 50,
            confirmations: 6,
        }
    }
}

/// Where a scan is, for progress display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanProgress {
    /// First block of the whole scan (the deployment block).
    pub start: u64,
    /// Last block scanned so far.
    pub scanned_to: u64,
    /// Block the current run is scanning up to.
    pub target: u64,
}

/// Notes found so far for one account on one chain, plus the checkpoint.
#[derive(Clone)]
pub struct ScanState {
    pub(crate) chain_id: u64,
    pub(crate) start_block: u64,
    pub(crate) next_block: u64,
    pub(crate) utxos: Vec<WalletUtxo>,
}

impl ScanState {
    /// Fresh state that scans from the deployment block.
    #[must_use]
    pub fn new(deployment: &RailgunDeployment) -> Self {
        Self {
            chain_id: deployment.chain_id,
            start_block: deployment.deployment_block,
            next_block: deployment.deployment_block,
            utxos: Vec::new(),
        }
    }

    /// Chain this state belongs to.
    #[must_use]
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Next block to scan.
    #[must_use]
    pub fn next_block(&self) -> u64 {
        self.next_block
    }

    /// All notes found (spent ones included).
    #[must_use]
    pub fn utxos(&self) -> &[WalletUtxo] {
        &self.utxos
    }

    /// Merge one decoded log batch: add new notes, then mark spends.
    pub(crate) fn apply(&mut self, delta: WalletLogDelta, nullifying_key: U256) {
        for utxo in delta.utxos {
            let known = self
                .utxos
                .iter()
                .any(|w| w.utxo.tree == utxo.tree && w.utxo.position == utxo.position);
            if !known {
                self.utxos.push(WalletUtxo::new(utxo));
            }
        }
        if delta.nullifiers.is_empty() {
            return;
        }
        let ours: HashMap<(u32, U256), usize> = self
            .utxos
            .iter()
            .enumerate()
            .filter(|(_, w)| !w.is_spent())
            .map(|(i, w)| ((w.utxo.tree, w.utxo.nullifier(nullifying_key)), i))
            .collect();
        for spent in delta.nullifiers {
            if let Some(&i) = ours.get(&(spent.tree, spent.nullifier)) {
                self.utxos[i].spent = Some(spent.source);
            }
        }
    }
}

/// Scan from `state.next_block()` to the confirmed head, updating `state`.
///
/// `on_chunk` runs after every chunk with the updated state, so callers can
/// show progress and checkpoint; returning `ControlFlow::Break` stops the
/// scan. `state` stays consistent up to the last finished chunk.
pub async fn scan<P: Provider>(
    provider: &P,
    deployment: &RailgunDeployment,
    account: &RailgunAccount,
    state: &mut ScanState,
    options: ScanOptions,
    mut on_chunk: impl FnMut(&ScanState, ScanProgress) -> ControlFlow<()>,
) -> Result<(), WalletError> {
    if state.chain_id != deployment.chain_id {
        return Err(WalletError::Other(
            "RAILGUN scan state is for a different chain".into(),
        ));
    }
    let head = provider
        .get_block_number()
        .await
        .map_err(|e| WalletError::RpcError(format!("block number: {e}")))?;
    check_history(provider, deployment).await?;
    let target = head.saturating_sub(options.confirmations);
    let topics = event_topics();
    let min_chunk = options.min_chunk.max(1);
    let mut chunk = options.chunk.max(min_chunk);

    while state.next_block <= target {
        let from = state.next_block;
        let to = from.saturating_add(chunk - 1).min(target);
        let filter = Filter::new()
            .address(deployment.contract)
            .event_signature(topics.clone())
            .from_block(from)
            .to_block(to);
        let logs = match provider.get_logs(&filter).await {
            Ok(logs) => logs,
            Err(_) if chunk > min_chunk => {
                chunk = (chunk / 2).max(min_chunk);
                continue;
            }
            Err(e) => {
                return Err(WalletError::RpcError(format!(
                    "RAILGUN logs {from}..={to}: {e}"
                )))
            }
        };
        if !logs.is_empty() {
            let delta = decode_with_timestamps(provider, &logs, account).await?;
            state.apply(delta, account.viewing().nullifying_key());
        }
        state.next_block = to + 1;
        chunk = chunk.saturating_mul(2).min(options.chunk.max(min_chunk));
        let progress = ScanProgress {
            start: state.start_block,
            scanned_to: to,
            target,
        };
        if on_chunk(state, progress).is_break() {
            break;
        }
    }
    Ok(())
}

/// Blocks after deployment that must contain the proxy's setup events.
const HISTORY_PROBE_BLOCKS: u64 = 50;

/// Fail unless the RPC serves logs from the deployment block.
///
/// Some public RPCs (e.g. PublicNode on Sepolia) answer old `eth_getLogs`
/// ranges with an empty list instead of an error, which would read as a zero
/// balance. Every deployment emits proxy setup events in its first blocks.
pub async fn check_history<P: Provider>(
    provider: &P,
    deployment: &RailgunDeployment,
) -> Result<(), WalletError> {
    let filter = Filter::new()
        .address(deployment.contract)
        .from_block(deployment.deployment_block)
        .to_block(deployment.deployment_block + HISTORY_PROBE_BLOCKS - 1);
    let logs = provider
        .get_logs(&filter)
        .await
        .map_err(|e| WalletError::RpcError(format!("RAILGUN history check: {e}")))?;
    if logs.is_empty() {
        return Err(WalletError::RpcError(
            "this RPC doesn't serve old logs (RAILGUN history missing) — use a full-history RPC"
                .into(),
        ));
    }
    Ok(())
}

/// Decode `logs`, fetching block timestamps only for blocks that touch our
/// notes (the parser needs one per log; most logs aren't ours).
pub(crate) async fn decode_with_timestamps<P: Provider>(
    provider: &P,
    logs: &[Log],
    account: &RailgunAccount,
) -> Result<WalletLogDelta, WalletError> {
    let placeholder: HashMap<u64, u64> = logs
        .iter()
        .filter_map(|l| Some((l.block_number?, l.block_timestamp.unwrap_or(0))))
        .collect();
    let mut delta = decode(logs, &placeholder, account)?;

    // Nullifiers aren't filtered to ours, so their timestamps stay as given.
    let missing: BTreeSet<u64> = delta
        .utxos
        .iter()
        .map(|u| &u.source)
        .filter(|s| s.block_timestamp == 0)
        .map(|s| s.block_number)
        .collect();
    let mut fetched = HashMap::with_capacity(missing.len());
    for block in missing {
        let header = provider
            .get_block_by_number(block.into())
            .await
            .map_err(|e| WalletError::RpcError(format!("block {block}: {e}")))?
            .ok_or_else(|| WalletError::RpcError(format!("block {block} not found")))?;
        fetched.insert(block, header.header.timestamp);
    }
    for utxo in &mut delta.utxos {
        if let Some(&ts) = fetched.get(&utxo.source.block_number) {
            utxo.source.block_timestamp = ts;
        }
    }
    Ok(delta)
}

fn decode(
    logs: &[Log],
    timestamps: &HashMap<u64, u64>,
    account: &RailgunAccount,
) -> Result<WalletLogDelta, WalletError> {
    parse_wallet_delta_from_logs(logs, timestamps, account.viewing().scan_keys())
        .map_err(|e| WalletError::Other(format!("RAILGUN log decode: {e}")))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use alloy::primitives::{aliases::U120, Address, FixedBytes, LogData};
    use alloy::transports::mock::Asserter;
    use broadcaster_core::contracts::railgun::TokenData;
    use broadcaster_core::contracts::shield::build_shield_request;
    use vaughan_core::security::hd_wallet::validate_mnemonic;

    use crate::balance::balances;
    use crate::deployment::{for_chain, SEPOLIA_CHAIN_ID};

    const MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    pub(crate) const TOKEN: Address = Address::repeat_byte(0x7a);

    pub(crate) fn account(index: u32) -> RailgunAccount {
        let seed = validate_mnemonic(MNEMONIC).unwrap().to_seed("");
        RailgunAccount::from_seed(&seed, index).unwrap()
    }

    fn rpc_log(data: LogData, block: u64) -> Log {
        Log {
            inner: alloy::primitives::Log {
                address: for_chain(SEPOLIA_CHAIN_ID).unwrap().contract,
                data,
            },
            block_hash: Some(B256::repeat_byte(0xbb)),
            block_number: Some(block),
            block_timestamp: Some(1_700_000_000 + block),
            transaction_hash: Some(B256::repeat_byte(0xcc)),
            transaction_index: Some(0),
            log_index: Some(0),
            removed: false,
        }
    }

    /// A real `Shield` log paying `value` of [`TOKEN`] to `to`, at tree 0 `position`.
    pub(crate) fn shield_log(to: &RailgunAccount, value: u64, position: u64, block: u64) -> Log {
        let keys = to.viewing().scan_keys();
        let request = build_shield_request(
            keys.master_public_key,
            &keys.viewing_public_key,
            TokenData::erc20(TOKEN),
            U120::from(value),
            &[7u8; 32],
        )
        .unwrap();
        let event = Shield {
            treeNumber: U256::ZERO,
            startPosition: U256::from(position),
            commitments: vec![request.preimage],
            shieldCiphertext: vec![request.ciphertext],
            fees: vec![U256::ZERO],
        };
        rpc_log(event.encode_log_data(), block)
    }

    fn nullified_log(nullifier: U256, block: u64) -> Log {
        let event = Nullified {
            treeNumber: 0,
            nullifier: vec![FixedBytes::from(nullifier.to_be_bytes::<32>())],
        };
        rpc_log(event.encode_log_data(), block)
    }

    fn sepolia_state() -> ScanState {
        ScanState::new(&for_chain(SEPOLIA_CHAIN_ID).unwrap())
    }

    #[test]
    fn shield_then_nullify_round_trip() {
        let me = account(0);
        let mut state = sepolia_state();
        let shield = shield_log(&me, 1_000, 5, 5_800_000);
        let delta = decode(
            std::slice::from_ref(&shield),
            &HashMap::from([(5_800_000, 1)]),
            &me,
        )
        .unwrap();
        state.apply(delta, me.viewing().nullifying_key());

        let b = balances(&state);
        assert_eq!(b.tokens.len(), 1);
        assert_eq!(b.tokens[0].token, TOKEN);
        assert_eq!(b.tokens[0].amount, U256::from(1_000));

        // Replaying the same log doesn't double count.
        let again = decode(&[shield], &HashMap::from([(5_800_000, 1)]), &me).unwrap();
        state.apply(again, me.viewing().nullifying_key());
        assert_eq!(balances(&state).tokens[0].amount, U256::from(1_000));

        let nullifier = state.utxos()[0]
            .utxo
            .nullifier(me.viewing().nullifying_key());
        let spend = decode(
            &[nullified_log(nullifier, 5_800_010)],
            &HashMap::from([(5_800_010, 2)]),
            &me,
        )
        .unwrap();
        state.apply(spend, me.viewing().nullifying_key());
        assert!(state.utxos()[0].is_spent());
        assert!(balances(&state).tokens.is_empty());
    }

    #[test]
    fn ignores_notes_for_other_accounts() {
        let me = account(0);
        let other = account(1);
        let mut state = sepolia_state();
        let delta = decode(
            &[shield_log(&other, 500, 0, 5_800_000)],
            &HashMap::from([(5_800_000, 1)]),
            &me,
        )
        .unwrap();
        state.apply(delta, me.viewing().nullifying_key());
        assert!(state.utxos().is_empty());
    }

    #[test]
    fn foreign_nullifier_leaves_note_unspent() {
        let me = account(0);
        let mut state = sepolia_state();
        let delta = decode(
            &[
                shield_log(&me, 42, 3, 5_800_000),
                nullified_log(U256::from(12_345), 5_800_000),
            ],
            &HashMap::from([(5_800_000, 1)]),
            &me,
        )
        .unwrap();
        state.apply(delta, me.viewing().nullifying_key());
        assert_eq!(balances(&state).tokens[0].amount, U256::from(42));
    }

    #[tokio::test]
    async fn scan_checkpoints_and_retries_smaller_chunks() {
        let me = account(0);
        let deployment = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let start = deployment.deployment_block;
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());

        // Head leaves 200 confirmed blocks; chunk 200 fails once, then 100 + 100.
        asserter.push_success(&format!("{:#x}", start + 199 + 6));
        asserter.push_success(&vec![nullified_log(U256::from(1), start + 2)]);
        asserter.push_failure_msg("range too large");
        asserter.push_success(&vec![shield_log(&me, 9, 0, start + 10)]);
        asserter.push_success(&Vec::<Log>::new());

        let mut state = ScanState::new(&deployment);
        let mut seen = Vec::new();
        let options = ScanOptions {
            chunk: 200,
            min_chunk: 50,
            confirmations: 6,
        };
        scan(&provider, &deployment, &me, &mut state, options, |_, p| {
            seen.push(p.scanned_to);
            ControlFlow::Continue(())
        })
        .await
        .unwrap();

        assert_eq!(seen, vec![start + 99, start + 199]);
        assert_eq!(state.next_block(), start + 200);
        assert_eq!(balances(&state).tokens[0].amount, U256::from(9));
        assert_eq!(
            state.utxos()[0].utxo.source.block_timestamp,
            1_700_000_000 + start + 10
        );
    }

    #[tokio::test]
    async fn scan_reports_error_at_min_chunk() {
        let me = account(0);
        let deployment = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        asserter.push_success(&format!("{:#x}", deployment.deployment_block + 1_000));
        asserter.push_success(&vec![nullified_log(
            U256::from(1),
            deployment.deployment_block,
        )]);
        asserter.push_failure_msg("down");
        let mut state = ScanState::new(&deployment);
        let options = ScanOptions {
            chunk: 50,
            min_chunk: 50,
            confirmations: 0,
        };
        let err = scan(&provider, &deployment, &me, &mut state, options, |_, _| {
            ControlFlow::Continue(())
        })
        .await
        .unwrap_err();
        assert!(matches!(err, WalletError::RpcError(_)));
        assert_eq!(state.next_block(), deployment.deployment_block);
    }

    /// Live: real Sepolia RAILGUN logs decode cleanly and the scanner runs.
    /// RPC from `RAILGUN_SEPOLIA_RPC` (default Tenderly; PublicNode drops old logs).
    #[tokio::test]
    #[ignore = "network: Sepolia RPC"]
    async fn live_sepolia_logs_decode() {
        let url = std::env::var("RAILGUN_SEPOLIA_RPC")
            .unwrap_or_else(|_| "https://sepolia.gateway.tenderly.co".into());
        let provider = connect(&url).unwrap();
        let deployment = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let me = account(0);

        // Busy window (Blockscout: ~400 Shield/Transact/Nullified events).
        let mut logs = Vec::new();
        for from in [9_080_000, 9_085_000] {
            let filter = Filter::new()
                .address(deployment.contract)
                .event_signature(event_topics())
                .from_block(from)
                .to_block(from + 4_999);
            logs.extend(provider.get_logs(&filter).await.unwrap());
        }
        assert!(logs.len() >= 100, "only {} RAILGUN logs", logs.len());
        let delta = decode_with_timestamps(&provider, &logs, &me).await.unwrap();
        println!(
            "{} logs decoded: {} of ours, {} nullifiers",
            logs.len(),
            delta.utxos.len(),
            delta.nullifiers.len()
        );
        check_history(&provider, &deployment).await.unwrap();

        let mut state = ScanState::new(&deployment);
        let head = provider.get_block_number().await.unwrap();
        state.next_block = head.saturating_sub(20_000);
        scan(
            &provider,
            &deployment,
            &me,
            &mut state,
            ScanOptions::default(),
            |_, _| ControlFlow::Continue(()),
        )
        .await
        .unwrap();
        assert!(state.next_block() > head.saturating_sub(10));
    }

    /// Live, slow: full Sepolia scan from the deployment block, timed.
    #[tokio::test]
    #[ignore = "network: Sepolia RPC, several minutes"]
    async fn live_sepolia_full_scan() {
        let url = std::env::var("RAILGUN_SEPOLIA_RPC")
            .unwrap_or_else(|_| "https://sepolia.gateway.tenderly.co".into());
        let provider = connect(&url).unwrap();
        let deployment = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let mut state = ScanState::new(&deployment);
        let started = std::time::Instant::now();
        scan(
            &provider,
            &deployment,
            &account(0),
            &mut state,
            ScanOptions::default(),
            |_, p| {
                if p.scanned_to % 500_000 < 5_000 {
                    println!(
                        "{} / {} after {:?}",
                        p.scanned_to,
                        p.target,
                        started.elapsed()
                    );
                }
                ControlFlow::Continue(())
            },
        )
        .await
        .unwrap();
        let spent = state.utxos().iter().filter(|u| u.is_spent()).count();
        println!(
            "full scan to {} in {:?}: {} notes ({spent} spent)",
            state.next_block(),
            started.elapsed(),
            state.utxos().len()
        );
        for t in balances(&state).tokens {
            println!("  {:#x}: {} raw in {} note(s)", t.token, t.amount, t.notes);
        }
    }

    #[tokio::test]
    async fn callback_can_stop_the_scan() {
        let me = account(0);
        let deployment = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let start = deployment.deployment_block;
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        asserter.push_success(&format!("{:#x}", start + 1_000));
        asserter.push_success(&vec![nullified_log(U256::from(1), start)]);
        asserter.push_success(&Vec::<Log>::new());
        let mut state = ScanState::new(&deployment);
        let options = ScanOptions {
            chunk: 100,
            min_chunk: 50,
            confirmations: 0,
        };
        scan(&provider, &deployment, &me, &mut state, options, |_, _| {
            ControlFlow::Break(())
        })
        .await
        .unwrap();
        assert_eq!(state.next_block(), start + 100);
    }

    #[tokio::test]
    async fn refuses_rpc_without_history() {
        let me = account(0);
        let deployment = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        asserter.push_success(&format!("{:#x}", deployment.deployment_block + 1_000));
        asserter.push_success(&Vec::<Log>::new());
        let mut state = ScanState::new(&deployment);
        let err = scan(
            &provider,
            &deployment,
            &me,
            &mut state,
            ScanOptions::default(),
            |_, _| ControlFlow::Continue(()),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("old logs"), "{err}");
    }

    #[tokio::test]
    async fn rejects_state_from_other_chain() {
        let me = account(0);
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let mut state = ScanState::new(&for_chain(1).unwrap());
        let sepolia = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        assert!(scan(
            &provider,
            &sepolia,
            &me,
            &mut state,
            ScanOptions::default(),
            |_, _| ControlFlow::Continue(())
        )
        .await
        .is_err());
    }
}
