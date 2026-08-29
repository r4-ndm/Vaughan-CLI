//! Curated per-chain ERC-20 registry used by auto asset detection
//! ([`crate::chains::evm::EvmAdapter::get_assets`]).
//!
//! Address provenance (see `docs/optimizations.md`): every entry was
//! **verified on-chain** (`symbol()`/`decimals()` via `cast` against the
//! chain's public RPC) on 2026-08-18, and cross-checked against the chain's
//! block explorer token search. WPLS additionally matches the canonical
//! addresses recorded in the DEX project's `docs/addresses.md`.
//!
//! This list is a *seed* for detection: at scan time the adapter re-reads
//! symbol/decimals from the contract (cached) and only the address here is
//! trusted. Tokens outside the list can still be queried by raw address.

/// A known token: registry address (trusted) + display metadata (fallback —
/// on-chain `symbol()`/`decimals()` win when the contract provides them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenEntry {
    pub symbol: &'static str,
    pub name: &'static str,
    pub address: &'static str,
    /// Verified decimals; used only as a fallback when `decimals()` reverts.
    pub decimals: u8,
}

/// PulseChain mainnet (369) — verified 2026-08-18 against rpc.pulsechain.com
/// and api.scan.pulsechain.com.
pub fn pulsechain_mainnet_tokens() -> Vec<TokenEntry> {
    vec![
        TokenEntry {
            symbol: "WPLS",
            name: "Wrapped Pulse",
            address: "0xA1077a294dDE1B09bB078844df40758a5D0f9a27",
            decimals: 18,
        },
        // NB: PulseChain HEX is at 0x…B7629768c40Eeb39, NOT the Ethereum
        // origin 0x…B4D7723a4c8D3f0e (the copy-on-fork deployed to a
        // different address).
        TokenEntry {
            symbol: "HEX",
            name: "HEX",
            address: "0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39",
            decimals: 8,
        },
        TokenEntry {
            symbol: "PLSX",
            name: "PulseX",
            address: "0x95B303987A60C71504D99Aa1b13B4DA07b0790ab",
            decimals: 18,
        },
        TokenEntry {
            symbol: "INC",
            name: "Internet Coin",
            address: "0x2fa878Ab3F87CC1C9737Fc071108F904c0B0C95d",
            decimals: 18,
        },
        // USDT retained the Ethereum fork copy at the origin address on
        // PulseChain; liquidity varies — verify before large trades.
        TokenEntry {
            symbol: "USDT",
            name: "Tether USD",
            address: "0xdAC17F958D2ee523a2206206994597C13D831ec7",
            decimals: 6,
        },
        // Bridged from Ethereum (Omnibridge/Liberty path) — NOT the inactive
        // fork copy at 0xA0b86991… (pUSDC label on 9X). Same economic value
        // as Ethereum USDC; verified on PulseChain scan + bridge client.
        TokenEntry {
            symbol: "USDC",
            name: "USD Coin from Ethereum",
            address: "0x15D38573d2feeb82e7ad5187aB8c1D52810B1f07",
            decimals: 6,
        },
    ]
}

/// PulseChain testnet v4 (943) — WPLS verified 2026-08-18 (matches the DEX
/// project's `docs/addresses.md`); WZRD smoke token for wiz4rd-swap testing.
pub fn pulsechain_testnet_tokens() -> Vec<TokenEntry> {
    use crate::core::wiz4rd::WZRD_SMOKE_943;
    vec![
        TokenEntry {
            symbol: "WPLS",
            name: "Wrapped Pulse",
            address: "0x70499adEBB11Efd915E3b69E700c331778628707",
            decimals: 18,
        },
        TokenEntry {
            symbol: "WZRD",
            name: "Wizard",
            address: WZRD_SMOKE_943,
            decimals: 18,
        },
    ]
}

/// The curated token list for a chain id (empty for chains with no registry —
/// native balance still works).
pub fn tokens_for_chain(chain_id: u64) -> Vec<TokenEntry> {
    match chain_id {
        369 => pulsechain_mainnet_tokens(),
        943 => pulsechain_testnet_tokens(),
        _ => Vec::new(),
    }
}

/// Find a registry entry by contract address (case-insensitive).
pub fn find_token(chain_id: u64, address: &str) -> Option<TokenEntry> {
    let needle = address.trim().to_ascii_lowercase();
    tokens_for_chain(chain_id)
        .into_iter()
        .find(|t| t.address.to_ascii_lowercase() == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mainnet_list_has_verified_wpls() {
        let tokens = pulsechain_mainnet_tokens();
        let wpls = tokens.iter().find(|t| t.symbol == "WPLS").unwrap();
        assert_eq!(
            wpls.address.to_lowercase(),
            "0xa1077a294dde1b09bb078844df40758a5d0f9a27"
        );
        assert_eq!(wpls.decimals, 18);
        // HEX must NOT be the Ethereum origin address (it differs on PLS).
        let hex = tokens.iter().find(|t| t.symbol == "HEX").unwrap();
        assert_ne!(
            hex.address.to_lowercase(),
            "0x2b591e99afe9f32eaa6214f7b4d7723a4c8d3f0e"
        );
        let usdc = tokens.iter().find(|t| t.symbol == "USDC").unwrap();
        assert_eq!(
            usdc.address.to_ascii_lowercase(),
            "0x15d38573d2feeb82e7ad5187ab8c1d52810b1f07"
        );
        assert_ne!(
            usdc.address.to_ascii_lowercase(),
            "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
        );
    }

    #[test]
    fn testnet_list_has_wpls_and_wzrd() {
        let tokens = pulsechain_testnet_tokens();
        assert_eq!(tokens.len(), 2);
        let wpls = tokens.iter().find(|t| t.symbol == "WPLS").unwrap();
        assert_eq!(
            wpls.address.to_lowercase(),
            "0x70499adebb11efd915e3b69e700c331778628707"
        );
        let wzrd = tokens.iter().find(|t| t.symbol == "WZRD").unwrap();
        assert_eq!(
            wzrd.address.to_lowercase(),
            "0x29bab93456c0e97ee931c1554c7c215480aa7766"
        );
    }

    #[test]
    fn unknown_chain_has_no_registry() {
        assert!(tokens_for_chain(56).is_empty());
        assert!(find_token(56, "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c").is_none());
    }

    #[test]
    fn find_token_is_case_insensitive() {
        let t = find_token(369, "0xA1077A294DDE1B09BB078844DF40758A5D0F9A27").unwrap();
        assert_eq!(t.symbol, "WPLS");
    }
}
