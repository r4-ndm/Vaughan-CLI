//! Allowlisted JSON-RPC read methods forwarded to the host's active network RPC.
//!
//! dApp-browser / Freedom pages call `eth_call`, `eth_estimateGas`, etc. through
//! the EIP-1193 provider. Routing reads via Vaughan (instead of the page's own
//! Infura key) reduces spoofed balance UX and keeps RPC on the user's network.

/// Returns true when `method` may be transparently forwarded to chain RPC.
pub fn is_read_proxy_method(method: &str) -> bool {
    matches!(
        method,
        "eth_call"
            | "eth_estimateGas"
            | "eth_getBalance"
            | "eth_blockNumber"
            | "eth_getTransactionCount"
            | "eth_getCode"
            | "eth_getStorageAt"
            | "eth_gasPrice"
            | "eth_feeHistory"
            | "eth_maxPriorityFeePerGas"
            | "eth_getBlockByNumber"
            | "eth_getBlockByHash"
            | "eth_getTransactionByHash"
            | "eth_getTransactionReceipt"
            | "eth_getLogs"
            | "net_version"
            | "web3_clientVersion"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_common_reads() {
        assert!(is_read_proxy_method("eth_call"));
        assert!(is_read_proxy_method("eth_estimateGas"));
        assert!(is_read_proxy_method("eth_getBalance"));
    }

    #[test]
    fn blocks_writes_and_admin() {
        assert!(!is_read_proxy_method("eth_sendRawTransaction"));
        assert!(!is_read_proxy_method("debug_traceTransaction"));
        assert!(!is_read_proxy_method("personal_sign"));
    }
}
