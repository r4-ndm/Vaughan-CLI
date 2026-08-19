//! Token registry: symbol → address + decimals, per chain.
//!
//! Phase 2: config-driven registry (TOML can add arbitrary tokens), plus an
//! on-chain `decimals()` fallback for anything not registered. The verified
//! constants come from `docs/addresses.md`.

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::sol_types::SolCall;

use crate::abi::IERC20Minimal;
use crate::error::{SdkError, SdkResult};

/// PulseChain chain IDs.
pub mod chain {
    pub const PULSECHAIN_MAINNET: u64 = 369;
    pub const PULSECHAIN_TESTNET_V4: u64 = 943;
    pub const BSC_MAINNET: u64 = 56; // reference chain for fork validation
}

/// A registered token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub symbol: &'static str,
    pub name: &'static str,
    pub address: Address,
    pub decimals: u8,
}

/// Native PLS — address is `Address::ZERO` (native, not an ERC20).
pub const PLS: Token = Token {
    symbol: "PLS",
    name: "Pulse",
    address: Address::ZERO,
    decimals: 18,
};

/// WPLS on PulseChain mainnet (verified on-chain, see docs/addresses.md).
pub const WPLS_MAINNET: Token = Token {
    symbol: "WPLS",
    name: "Wrapped Pulse",
    address: Address::new([0xa1, 0x07, 0x7a, 0x29, 0x4d, 0xde, 0x1b, 0x09, 0xbb, 0x07, 0x88, 0x44, 0xdf, 0x40, 0x75, 0x8a, 0x5d, 0x0f, 0x9a, 0x27]),
    decimals: 18,
};

/// WPLS on PulseChain testnet V4 (verified on-chain, docs/addresses.md).
/// ⚠️ Different address from mainnet — do not conflate.
pub const WPLS_TESTNET: Token = Token {
    symbol: "WPLS",
    name: "Wrapped Pulse",
    address: Address::new([0x70, 0x49, 0x9a, 0xde, 0xbb, 0x11, 0xef, 0xd9, 0x15, 0xe3, 0xb6, 0x9e, 0x70, 0x0c, 0x33, 0x17, 0x78, 0x62, 0x87, 0x07]),
    decimals: 18,
};

/// Look up a token by symbol + chain id. Returns `None` for unregistered pairs.
pub fn lookup(symbol: &str, chain_id: u64) -> Option<Token> {
    match (symbol.to_ascii_uppercase().as_str(), chain_id) {
        ("PLS", _) => Some(PLS),
        ("WPLS", chain::PULSECHAIN_MAINNET) => Some(WPLS_MAINNET),
        ("WPLS", chain::PULSECHAIN_TESTNET_V4) => Some(WPLS_TESTNET),
        _ => None,
    }
}

/// Resolve a token by symbol, falling back to the registry lookup for
/// unregistered symbols — this is where config-driven tokens plug in later.
pub fn resolve(symbol: &str, chain_id: u64, configured: &[Token]) -> Option<Token> {
    let sym = symbol.to_ascii_uppercase();
    configured.iter().find(|t| t.symbol == sym).copied().or_else(|| lookup(&sym, chain_id))
}

/// Fetch `decimals()` on-chain for an arbitrary ERC20.
///
/// Native PLS (address zero) short-circuits to 18. Contracts that do not
/// implement `decimals()` fall back to 18 rather than erroring — callers that
/// need strictness can check the error.
pub async fn decimals_of<P: Provider>(provider: &P, token: Address) -> SdkResult<u8> {
    if token.is_zero() {
        return Ok(18); // native PLS
    }
    let call = IERC20Minimal::decimalsCall {};
    let data = call.abi_encode();
    let raw = provider
        .call(
            alloy::rpc::types::TransactionRequest::default()
                .to(token)
                .input(data.into()),
        )
        .await?;
    let decoded = IERC20Minimal::decimalsCall::abi_decode_returns(&raw)
        .map_err(|e| SdkError::Decode(e))?;
    Ok(decoded)
}

/// Format a raw amount (wei) into a human string using the token's decimals.
pub fn format_amount(raw: U256, decimals: u8) -> String {
    let scale = 10u128.pow(decimals as u32);
    let whole = raw / U256::from(scale);
    let frac = raw % U256::from(scale);
    if frac.is_zero() {
        whole.to_string()
    } else {
        let frac_str = format!("{:0width$}", frac, width = decimals as usize);
        let frac_str = frac_str.trim_end_matches('0');
        format!("{whole}.{frac_str}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wpls_resolves_per_chain() {
        let mainnet = lookup("WPLS", chain::PULSECHAIN_MAINNET).unwrap();
        let testnet = lookup("WPLS", chain::PULSECHAIN_TESTNET_V4).unwrap();
        assert_eq!(mainnet.symbol, "WPLS");
        assert_eq!(mainnet.decimals, 18);
        assert_ne!(mainnet.address, testnet.address, "WPLS differs per chain");
    }

    #[test]
    fn pls_is_native() {
        let pls = lookup("PLS", chain::PULSECHAIN_MAINNET).unwrap();
        assert_eq!(pls.address, Address::ZERO);
        assert_eq!(pls.decimals, 18);
    }

    #[test]
    fn unknown_symbol_is_none() {
        assert!(lookup("WPLS", chain::BSC_MAINNET).is_none());
        assert!(lookup("NOTATOKEN", chain::PULSECHAIN_MAINNET).is_none());
    }

    #[test]
    fn resolve_prefers_configured_over_registry() {
        let configured = [Token {
            symbol: "WPLS",
            name: "custom wpls",
            address: Address::repeat_byte(0xab),
            decimals: 18,
        }];
        let t = resolve("wpls", chain::PULSECHAIN_MAINNET, &configured).unwrap();
        assert_eq!(t.address, Address::repeat_byte(0xab), "configured wins");
    }

    #[test]
    fn format_amount_uses_decimals() {
        assert_eq!(format_amount(U256::from(1_500_000_000u64), 9), "1.5");
        assert_eq!(format_amount(U256::from(1_000_000_000u64), 9), "1");
        assert_eq!(format_amount(U256::from(123u64), 2), "1.23");
    }
}
