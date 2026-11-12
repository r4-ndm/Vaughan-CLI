//! Per-token shielded balances from scanned notes.
//!
//! ERC-20 notes carry the token address as their token hash. NFT notes carry
//! a hash of the token data instead; they're counted but not valued here.
//! Spendability (POI status) isn't checked in Phase 1.

use std::collections::BTreeMap;

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::sol;

use crate::scan::ScanState;

sol! {
    #[sol(rpc)]
    interface IErc20Meta {
        function symbol() external view returns (string);
        function decimals() external view returns (uint8);
    }
}

/// ERC-20 `(symbol, decimals)` for display; `None` if the token doesn't answer.
pub async fn token_metadata<P: Provider>(provider: &P, token: Address) -> Option<(String, u8)> {
    let erc20 = IErc20Meta::new(token, provider);
    let decimals = erc20.decimals().call().await.ok()?;
    let symbol = erc20.symbol().call().await.ok()?;
    let symbol: String = symbol
        .chars()
        .filter(|c| !c.is_control())
        .take(16)
        .collect();
    Some((symbol, decimals))
}

/// Unspent shielded amount of one ERC-20 token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenBalance {
    pub token: Address,
    /// Raw amount in the token's smallest unit.
    pub amount: U256,
    /// Unspent notes making up `amount`.
    pub notes: usize,
}

/// Balances computed from a [`ScanState`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShieldedBalances {
    /// ERC-20 balances, sorted by token address.
    pub tokens: Vec<TokenBalance>,
    /// Unspent notes that aren't ERC-20 (NFTs), not valued.
    pub other_notes: usize,
}

/// Sum unspent notes per ERC-20 token.
#[must_use]
pub fn balances(state: &ScanState) -> ShieldedBalances {
    let mut by_token: BTreeMap<Address, TokenBalance> = BTreeMap::new();
    let mut other_notes = 0;
    for w in state.utxos().iter().filter(|w| !w.is_spent()) {
        let hash = w.utxo.note.token_hash.to_be_bytes::<32>();
        if hash[..12].iter().any(|b| *b != 0) {
            other_notes += 1;
            continue;
        }
        let token = Address::from_slice(&hash[12..]);
        let entry = by_token.entry(token).or_insert(TokenBalance {
            token,
            amount: U256::ZERO,
            notes: 0,
        });
        entry.amount = entry.amount.saturating_add(w.utxo.note.value);
        entry.notes += 1;
    }
    ShieldedBalances {
        tokens: by_token.into_values().collect(),
        other_notes,
    }
}
