//! Opt-in quick-sync: read RAILGUN history from the public RAILGUN indexer
//! ("squid") instead of millions of blocks of `eth_getLogs`.
//!
//! The indexer serves every commitment and nullifier, not a per-wallet view,
//! and notes are still decrypted locally, so it learns nothing about which
//! notes are ours. It does see our IP address and when we sync, so frontends
//! run it only when the user asks. Like an RPC it could hide notes, but it
//! can't forge notes to us. There is no indexer for Sepolia.

use std::ops::ControlFlow;

use merkletree::quick::{QuickSyncClient, DEFAULT_PAGE_SIZE};
use railgun_wallet::scan::{
    parse_indexed_wallet_delta, IndexedLegacyEncryptedCommitmentInput,
    IndexedLegacyGeneratedCommitmentInput, IndexedNullifierInput, IndexedShieldCommitmentInput,
    IndexedTransactCommitmentInput,
};
use vaughan_core::error::WalletError;

use crate::deployment::RailgunDeployment;
use crate::keys::RailgunAccount;
use crate::scan::{ScanProgress, ScanState};

/// `(chain id, indexer GraphQL endpoint, blocks per page)`, as used by
/// railgun-rust's sync service.
const ENDPOINTS: &[(u64, &str, u64)] = &[
    (
        1,
        "https://rail-squid.squids.live/squid-railgun-ethereum-v2/graphql",
        300_000,
    ),
    (
        56,
        "https://rail-squid.squids.live/squid-railgun-bsc-v2/graphql",
        1_000_000,
    ),
    (
        137,
        "https://rail-squid.squids.live/squid-railgun-polygon-v2/graphql",
        1_000_000,
    ),
    (
        42_161,
        "https://rail-squid.squids.live/squid-railgun-arbitrum-v2/graphql",
        5_000_000,
    ),
];

/// Indexer endpoint and page range for `chain_id`, if it has one.
#[must_use]
pub fn endpoint(chain_id: u64) -> Option<(&'static str, u64)> {
    ENDPOINTS
        .iter()
        .find(|(id, ..)| *id == chain_id)
        .map(|&(_, url, range)| (url, range))
}

/// True when quick-sync is available on `chain_id`.
#[must_use]
pub fn is_available(chain_id: u64) -> bool {
    endpoint(chain_id).is_some()
}

/// One block range of indexed rows, complete up to `checkpoint`.
#[derive(Default)]
pub struct Page {
    pub transact: Vec<IndexedTransactCommitmentInput>,
    pub shield: Vec<IndexedShieldCommitmentInput>,
    pub legacy_encrypted: Vec<IndexedLegacyEncryptedCommitmentInput>,
    pub legacy_generated: Vec<IndexedLegacyGeneratedCommitmentInput>,
    pub nullifiers: Vec<IndexedNullifierInput>,
    /// Last block whose rows are all in this page.
    pub checkpoint: u64,
}

/// Where indexed rows come from (the indexer, or a fake in tests).
#[allow(async_fn_in_trait)]
pub trait PageSource {
    /// Highest block the source has indexed.
    async fn height(&self) -> Result<u64, WalletError>;
    /// Rows for `from..=to`; `legacy` selects pre-v2 events.
    async fn page(&self, legacy: bool, from: u64, to: u64) -> Result<Page, WalletError>;
}

/// The public RAILGUN indexer for one chain.
pub struct Indexer {
    client: QuickSyncClient,
    range: u64,
}

impl Indexer {
    /// Indexer client for `chain_id`, or `None` where there is no indexer.
    #[must_use]
    pub fn for_chain(chain_id: u64) -> Option<Self> {
        let (url, range) = endpoint(chain_id)?;
        let url = url.parse().ok()?;
        Some(Self {
            client: QuickSyncClient::new(url),
            range,
        })
    }

    /// Blocks per page for this chain.
    #[must_use]
    pub fn range(&self) -> u64 {
        self.range
    }
}

fn indexer_error(e: impl std::fmt::Display) -> WalletError {
    WalletError::NetworkError(format!("RAILGUN indexer: {e}"))
}

impl PageSource for Indexer {
    async fn height(&self) -> Result<u64, WalletError> {
        self.client
            .fetch_squid_height()
            .await
            .map_err(indexer_error)
    }

    async fn page(&self, legacy: bool, from: u64, to: u64) -> Result<Page, WalletError> {
        let size = DEFAULT_PAGE_SIZE.get();
        if legacy {
            let p = self
                .client
                .fetch_indexed_legacy_wallet_page(from, to, DEFAULT_PAGE_SIZE)
                .await
                .map_err(indexer_error)?;
            let legacy_encrypted: Vec<IndexedLegacyEncryptedCommitmentInput> = p
                .legacy_encrypted_commitments
                .into_iter()
                .map(Into::into)
                .collect();
            let legacy_generated: Vec<IndexedLegacyGeneratedCommitmentInput> = p
                .legacy_generated_commitments
                .into_iter()
                .map(Into::into)
                .collect();
            let nullifiers: Vec<IndexedNullifierInput> =
                p.nullifiers.into_iter().map(Into::into).collect();
            let checkpoint = stream_checkpoint(
                size,
                to,
                legacy_encrypted.iter().map(|r| r.source.block_number),
            )
            .min(stream_checkpoint(
                size,
                to,
                legacy_generated.iter().map(|r| r.source.block_number),
            ))
            .min(stream_checkpoint(
                size,
                to,
                nullifiers.iter().map(|r| r.source.block_number),
            ));
            let mut page = Page {
                legacy_encrypted,
                legacy_generated,
                nullifiers,
                checkpoint,
                ..Page::default()
            };
            page.truncate();
            Ok(page)
        } else {
            let p = self
                .client
                .fetch_indexed_wallet_page(from, to, DEFAULT_PAGE_SIZE)
                .await
                .map_err(indexer_error)?;
            let transact: Vec<IndexedTransactCommitmentInput> =
                p.transact_commitments.into_iter().map(Into::into).collect();
            let shield: Vec<IndexedShieldCommitmentInput> =
                p.shield_commitments.into_iter().map(Into::into).collect();
            let nullifiers: Vec<IndexedNullifierInput> =
                p.nullifiers.into_iter().map(Into::into).collect();
            let checkpoint =
                stream_checkpoint(size, to, transact.iter().map(|r| r.source.block_number))
                    .min(stream_checkpoint(
                        size,
                        to,
                        shield.iter().map(|r| r.source.block_number),
                    ))
                    .min(stream_checkpoint(
                        size,
                        to,
                        nullifiers.iter().map(|r| r.source.block_number),
                    ));
            let mut page = Page {
                transact,
                shield,
                nullifiers,
                checkpoint,
                ..Page::default()
            };
            page.truncate();
            Ok(page)
        }
    }
}

/// Last block a stream fully covers: a full page may stop mid-block, so its
/// last block is left for the next page.
fn stream_checkpoint(page_size: usize, to: u64, blocks: impl ExactSizeIterator<Item = u64>) -> u64 {
    if blocks.len() < page_size {
        return to;
    }
    blocks.max().unwrap_or(to).saturating_sub(1)
}

impl Page {
    /// Newest block with any row in this page.
    fn last_row_block(&self) -> Option<u64> {
        let blocks = self
            .transact
            .iter()
            .map(|r| r.source.block_number)
            .chain(self.shield.iter().map(|r| r.source.block_number))
            .chain(self.legacy_encrypted.iter().map(|r| r.source.block_number))
            .chain(self.legacy_generated.iter().map(|r| r.source.block_number))
            .chain(self.nullifiers.iter().map(|r| r.source.block_number));
        blocks.max()
    }

    /// Drop rows after `checkpoint` (they are refetched with the next page).
    fn truncate(&mut self) {
        let last = self.checkpoint;
        self.transact.retain(|r| r.source.block_number <= last);
        self.shield.retain(|r| r.source.block_number <= last);
        self.legacy_encrypted
            .retain(|r| r.source.block_number <= last);
        self.legacy_generated
            .retain(|r| r.source.block_number <= last);
        self.nullifiers.retain(|r| r.source.block_number <= last);
    }
}

/// Bring `state` up to the indexer's height, page by page.
///
/// Leaves the remaining blocks to [`crate::scan::scan`] over RPC. `on_page`
/// works like `scan`'s `on_chunk`.
pub async fn quick_sync<S: PageSource>(
    source: &S,
    range: u64,
    deployment: &RailgunDeployment,
    account: &RailgunAccount,
    state: &mut ScanState,
    mut on_page: impl FnMut(&ScanState, ScanProgress) -> ControlFlow<()>,
) -> Result<(), WalletError> {
    if state.chain_id != deployment.chain_id {
        return Err(WalletError::Other(
            "RAILGUN scan state is for a different chain".into(),
        ));
    }
    let target = source.height().await?;
    let range = range.max(1);
    while state.next_block <= target {
        let from = state.next_block;
        let legacy = deployment.v2_start_block > 0 && from < deployment.v2_start_block;
        let mut to = from.saturating_add(range - 1).min(target);
        if legacy {
            to = to.min(deployment.v2_start_block - 1);
        }
        let page = source.page(legacy, from, to).await?;
        if page.checkpoint < from || page.checkpoint > to {
            return Err(indexer_error(format!(
                "incomplete page for blocks {from}..={to}"
            )));
        }
        let delta = parse_indexed_wallet_delta(
            &page.transact,
            &page.shield,
            &page.legacy_encrypted,
            &page.legacy_generated,
            &page.nullifiers,
            account.viewing().scan_keys(),
        );
        state.apply(delta, account.viewing().nullifying_key());
        state.note_log_block(page.last_row_block());
        state.next_block = page.checkpoint + 1;
        let progress = ScanProgress {
            start: state.start_block,
            scanned_to: page.checkpoint,
            target,
        };
        if on_page(state, progress).is_break() {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use alloy::primitives::{aliases::U120, B256, U256};
    use broadcaster_core::contracts::railgun::TokenData;
    use broadcaster_core::contracts::shield::build_shield_request;
    use broadcaster_core::utxo::UtxoSource;

    use super::*;
    use crate::balance::balances;
    use crate::deployment::{for_chain, SEPOLIA_CHAIN_ID};
    use crate::scan::tests::{account, TOKEN};

    /// In-memory indexer serving `page_size` rows per stream per call.
    struct Fake {
        height: u64,
        page_size: usize,
        shields: Vec<IndexedShieldCommitmentInput>,
        nullifiers: Vec<IndexedNullifierInput>,
        calls: RefCell<Vec<(bool, u64, u64)>>,
    }

    impl PageSource for Fake {
        async fn height(&self) -> Result<u64, WalletError> {
            Ok(self.height)
        }

        async fn page(&self, legacy: bool, from: u64, to: u64) -> Result<Page, WalletError> {
            self.calls.borrow_mut().push((legacy, from, to));
            let in_range = |b: u64| (from..=to).contains(&b);
            let shield: Vec<_> = self
                .shields
                .iter()
                .filter(|r| !legacy && in_range(r.source.block_number))
                .take(self.page_size)
                .cloned()
                .collect();
            let nullifiers: Vec<_> = self
                .nullifiers
                .iter()
                .filter(|r| in_range(r.source.block_number))
                .take(self.page_size)
                .cloned()
                .collect();
            let checkpoint = stream_checkpoint(
                self.page_size,
                to,
                shield.iter().map(|r| r.source.block_number),
            )
            .min(stream_checkpoint(
                self.page_size,
                to,
                nullifiers.iter().map(|r| r.source.block_number),
            ));
            let mut page = Page {
                shield,
                nullifiers,
                checkpoint,
                ..Page::default()
            };
            page.truncate();
            Ok(page)
        }
    }

    fn source(block: u64) -> UtxoSource {
        UtxoSource {
            tx_hash: B256::repeat_byte(0xcc),
            block_number: block,
            block_timestamp: 1_700_000_000,
        }
    }

    fn shield_row(
        to: &RailgunAccount,
        value: u64,
        position: u64,
        block: u64,
    ) -> IndexedShieldCommitmentInput {
        let keys = to.viewing().scan_keys();
        let request = build_shield_request(
            keys.master_public_key,
            &keys.viewing_public_key,
            TokenData::erc20(TOKEN),
            U120::from(value),
            &[7u8; 32],
        )
        .unwrap();
        IndexedShieldCommitmentInput {
            tree_number: 0,
            tree_position: position,
            preimage: request.preimage,
            shield_ciphertext: request.ciphertext,
            source: source(block),
        }
    }

    fn fake(height: u64, page_size: usize, shields: Vec<IndexedShieldCommitmentInput>) -> Fake {
        Fake {
            height,
            page_size,
            shields,
            nullifiers: Vec::new(),
            calls: RefCell::new(Vec::new()),
        }
    }

    #[tokio::test]
    async fn pages_through_full_pages_without_losing_rows() {
        let me = account(0);
        let deployment = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let start = deployment.deployment_block;
        // Page size 2: every page is full and stops before its last block.
        let shields = vec![
            shield_row(&me, 1, 0, start + 10),
            shield_row(&me, 2, 1, start + 11),
            shield_row(&me, 4, 2, start + 12),
            shield_row(&me, 8, 3, start + 13),
        ];
        let src = fake(start + 20, 2, shields);
        let mut state = ScanState::new(&deployment);
        let mut seen = Vec::new();
        quick_sync(&src, 1_000, &deployment, &me, &mut state, |_, p| {
            seen.push(p.scanned_to);
            ControlFlow::Continue(())
        })
        .await
        .unwrap();

        assert_eq!(balances(&state).tokens[0].amount, U256::from(15));
        assert_eq!(state.next_block(), start + 21);
        assert_eq!(state.last_log_block, Some(start + 13));
        assert_eq!(seen.last(), Some(&(start + 20)));
    }

    #[tokio::test]
    async fn block_larger_than_a_page_is_an_error() {
        let me = account(0);
        let deployment = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let start = deployment.deployment_block;
        let shields = (0..3).map(|i| shield_row(&me, 1, i, start + 5)).collect();
        let src = fake(start + 20, 2, shields);
        let mut state = ScanState::new(&deployment);
        let err = quick_sync(&src, 1_000, &deployment, &me, &mut state, |_, _| {
            ControlFlow::Continue(())
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("incomplete page"), "{err}");
    }

    #[tokio::test]
    async fn marks_notes_spent_from_indexed_nullifiers() {
        let me = account(0);
        let deployment = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let start = deployment.deployment_block;
        let mut src = fake(start + 50, 100, vec![shield_row(&me, 5, 0, start + 1)]);

        let mut state = ScanState::new(&deployment);
        quick_sync(&src, 10, &deployment, &me, &mut state, |_, _| {
            ControlFlow::Continue(())
        })
        .await
        .unwrap();
        let nullifier = state.utxos()[0]
            .utxo
            .nullifier(me.viewing().nullifying_key());

        src.height = start + 100;
        src.nullifiers.push(IndexedNullifierInput {
            tree_number: 0,
            nullifier,
            source: source(start + 70),
        });
        quick_sync(&src, 10, &deployment, &me, &mut state, |_, _| {
            ControlFlow::Continue(())
        })
        .await
        .unwrap();
        assert!(state.utxos()[0].is_spent());
        assert!(balances(&state).tokens.is_empty());
    }

    #[tokio::test]
    async fn uses_legacy_pages_before_v2_start() {
        let me = account(0);
        let mut deployment = for_chain(SEPOLIA_CHAIN_ID).unwrap();
        let start = deployment.deployment_block;
        deployment.v2_start_block = start + 100;
        let src = fake(start + 250, 100, Vec::new());
        let mut state = ScanState::new(&deployment);
        quick_sync(&src, 1_000, &deployment, &me, &mut state, |_, _| {
            ControlFlow::Continue(())
        })
        .await
        .unwrap();
        assert_eq!(
            src.calls.into_inner(),
            vec![(true, start, start + 99), (false, start + 100, start + 250)]
        );
    }

    #[test]
    fn indexers_cover_mainnets_only() {
        for id in [1, 56, 137, 42_161] {
            assert!(is_available(id), "chain {id}");
        }
        assert!(!is_available(SEPOLIA_CHAIN_ID));
        assert!(!is_available(369));
    }

    /// Live: the Ethereum indexer answers and a recent page decodes.
    #[tokio::test]
    #[ignore = "network: RAILGUN indexer"]
    async fn live_ethereum_indexer_page() {
        let indexer = Indexer::for_chain(1).unwrap();
        let height = indexer.height().await.unwrap();
        let from = height.saturating_sub(indexer.range());
        let page = indexer.page(false, from, height).await.unwrap();
        println!(
            "height {height}: {} transact, {} shield, {} nullifiers up to {}",
            page.transact.len(),
            page.shield.len(),
            page.nullifiers.len(),
            page.checkpoint
        );
        assert!(page.transact.len() + page.shield.len() + page.nullifiers.len() > 0);
    }
}
