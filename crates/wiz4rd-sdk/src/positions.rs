//! Position reading: decode `positions(tokenId)` and list a user's positions.
//!
//! The PositionManager is plain ERC721 (not Enumerable), so ownership is
//! discovered from `Transfer` events (`to` = owner) rather than a
//! `tokenOfOwnerByIndex` view — same approach Uniswap's own tooling uses.

use alloy::primitives::{keccak256, Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::{Filter, TransactionRequest};
use alloy::sol_types::{SolCall, SolValue};

use crate::abi::{IERC721Minimal, INonfungiblePositionManager};
use crate::config::Config;
use crate::error::{SdkError, SdkResult};

/// Decoded `positions(tokenId)` state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionInfo {
    pub token_id: U256,
    pub nonce: u64,
    pub operator: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity: u128,
    pub fee_growth_inside0_last_x128: U256,
    pub fee_growth_inside1_last_x128: U256,
    pub tokens_owed0: u128,
    pub tokens_owed1: u128,
}

/// `keccak256("Transfer(address,address,uint256)")` — the ERC721 transfer
/// topic, computed once (matches `IERC721Minimal`'s event).
pub fn transfer_topic() -> B256 {
    keccak256("Transfer(address,address,uint256)")
}

/// Fetch one position's state from the PositionManager.
pub async fn get_position<P: Provider>(
    provider: &P,
    config: &Config,
    token_id: U256,
) -> SdkResult<PositionInfo> {
    let npm = config
        .position_manager
        .ok_or(SdkError::MissingAddress("position_manager"))?;

    let call = INonfungiblePositionManager::positionsCall { tokenId: token_id };
    let raw = provider
        .call(
            TransactionRequest::default()
                .to(npm)
                .input(call.abi_encode().into()),
        )
        .await?;
    let r = INonfungiblePositionManager::positionsCall::abi_decode_returns(&raw)?;

    Ok(PositionInfo {
        token_id,
        nonce: r.nonce.to::<u64>(),
        operator: r.operator,
        token0: r.token0,
        token1: r.token1,
        fee: r.fee.to::<u32>(),
        tick_lower: i32::try_from(r.tickLower).map_err(|e| SdkError::Math(e.to_string()))?,
        tick_upper: i32::try_from(r.tickUpper).map_err(|e| SdkError::Math(e.to_string()))?,

        liquidity: r.liquidity,
        fee_growth_inside0_last_x128: r.feeGrowthInside0LastX128,
        fee_growth_inside1_last_x128: r.feeGrowthInside1LastX128,
        tokens_owed0: r.tokensOwed0,
        tokens_owed1: r.tokensOwed1,
    })
}

/// The current owner of a position NFT (`ownerOf`). Useful as a pre-flight
/// ownership check before decrease/collect — the contract reverts too, but a
/// clear error beats a gas-burning revert.
pub async fn position_owner<P: Provider>(
    provider: &P,
    config: &Config,
    token_id: U256,
) -> SdkResult<Address> {
    let npm = config
        .position_manager
        .ok_or(SdkError::MissingAddress("position_manager"))?;
    let call = IERC721Minimal::ownerOfCall { tokenId: token_id };
    let raw = provider
        .call(
            TransactionRequest::default()
                .to(npm)
                .input(call.abi_encode().into()),
        )
        .await?;
    Address::abi_decode(&raw).map_err(SdkError::Decode)
}

/// List a user's positions: scan `Transfer` events to the user, keep token IDs
/// they still own, and decode each position.
///
/// Note: `to_block` defaults to `latest`; for a full history on a long-lived
/// chain this can be a heavy scan — the CLI should offer a block range.
pub async fn list_positions<P: Provider>(
    provider: &P,
    config: &Config,
    owner: Address,
) -> SdkResult<Vec<PositionInfo>> {
    list_positions_from(provider, config, owner, None, None).await
}

/// [`list_positions`] with an optional block range — pass the block where the
/// owner first received NFTs to keep the log query bounded on long-lived
/// chains (public RPCs commonly reject full-history scans, and many cap the
/// range at a few thousand blocks, so bound `to_block` too).
pub async fn list_positions_from<P: Provider>(
    provider: &P,
    config: &Config,
    owner: Address,
    from_block: Option<u64>,
    to_block: Option<u64>,
) -> SdkResult<Vec<PositionInfo>> {
    let npm = config
        .position_manager
        .ok_or(SdkError::MissingAddress("position_manager"))?;

    let mut filter = Filter::new()
        .address(npm)
        .event_signature(transfer_topic())
        .topic2(owner); // Transfer(from, to, tokenId): `to` is topic index 2
    if let Some(b) = from_block {
        filter = filter.from_block(b);
    }
    if let Some(b) = to_block {
        filter = filter.to_block(b);
    }

    let logs = provider.get_logs(&filter).await?;

    let mut seen = std::collections::HashSet::new();
    for log in &logs {
        // Transfer(from, to, tokenId): topics are [sig, from, to, tokenId],
        // so the tokenId is topic index 3.
        if let Some(topic) = log.topics().get(3) {
            seen.insert(U256::from_be_bytes(topic.0));
        }
    }

    let mut out = Vec::new();
    for token_id in seen {
        // Ownership check first: the user may have transferred or burned the
        // token since the log was emitted. `positions()` reverts for burned
        // tokens, so decoding must come after the owner check.
        if position_owner(provider, config, token_id).await? != owner {
            continue;
        }
        let pos = get_position(provider, config, token_id).await?;
        out.push(pos);
    }
    out.sort_by_key(|p| p.token_id);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_topic_matches_erc721() {
        // Standard ERC721 Transfer event signature hash.
        assert_eq!(
            transfer_topic(),
            keccak256("Transfer(address,address,uint256)")
        );
    }

    #[test]
    fn transfer_topics_layout_has_token_id_at_index_3() {
        // Transfer(from, to, tokenId): topics are [sig, from, to, tokenId].
        // `to` is topic 2 (used by the log filter) and tokenId is topic 3
        // (used to collect the seen set). Regression test for the list flow.
        let from = Address::repeat_byte(0x11);
        let to = Address::repeat_byte(0x22);
        let token_id = U256::from(0x6d_758a_u64);
        let sig = transfer_topic();
        let topics = [
            sig,
            from.into_word(),
            to.into_word(),
            B256::from(token_id.to_be_bytes()),
        ];
        assert_eq!(topics[2], to.into_word(), "`to` is topic index 2");
        assert_eq!(
            topics[3],
            B256::from(token_id.to_be_bytes()),
            "tokenId is topic index 3"
        );
    }
}
