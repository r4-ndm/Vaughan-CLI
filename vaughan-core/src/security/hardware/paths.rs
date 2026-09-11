//! Shared BIP-44 path strings for EVM hardware (Ledger Live / standard / legacy).
//!
//! Vendor crates parse these strings themselves — keep this module free of HID
//! and Alloy `HDPath` so Trezor and Ledger stay interchangeable at the UI layer.

/// Ledger Live / Trezor “account” style: `m/44'/60'/{account}'/0/0`.
pub fn evm_ledger_live_path(account: u32) -> String {
    format!("m/44'/60'/{account}'/0/0")
}

/// Standard BIP-44 external chain: `m/44'/60'/0'/0/{index}` (MetaMask-family).
pub fn evm_standard_path(index: u32) -> String {
    format!("m/44'/60'/0'/0/{index}")
}

/// Legacy Ledger path: `m/44'/60'/0'/{index}`.
pub fn evm_legacy_path(index: u32) -> String {
    format!("m/44'/60'/0'/{index}")
}

/// Default preview set for “Add device” (Live accounts `0..count`).
pub fn evm_live_preview_paths(count: usize) -> Vec<String> {
    (0..count).map(|i| evm_ledger_live_path(i as u32)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_and_standard_paths() {
        assert_eq!(evm_ledger_live_path(0), "m/44'/60'/0'/0/0");
        assert_eq!(evm_ledger_live_path(2), "m/44'/60'/2'/0/0");
        assert_eq!(evm_standard_path(3), "m/44'/60'/0'/0/3");
        assert_eq!(evm_legacy_path(1), "m/44'/60'/0'/1");
        assert_eq!(evm_live_preview_paths(2).len(), 2);
    }
}
