//! Calldata construction inside the signer daemon.
//!
//! # Security Invariant
//! All calldata bytes are generated internally from verified, re-derived parameters.
//! No calldata ever crosses the IPC socket from the caller.

use alloy::primitives::aliases::{U160, U24};
use alloy::primitives::{Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::BatchTransferLeg;

sol! {
    interface IERC20Transfer {
        function transfer(address to, uint256 amount) external returns (bool);
    }

    interface IERC20Approve {
        function approve(address spender, uint256 amount) external returns (bool);
    }

    interface IWETH9 {
        function deposit() external payable;
        function withdraw(uint256 wad) external;
    }

    interface IUniswapV2RouterSwap {
        function swapExactTokensForTokens(
            uint256 amountIn,
            uint256 amountOutMin,
            address[] calldata path,
            address to,
            uint256 deadline
        ) external returns (uint256[] memory amounts);
    }

    interface ISwapRouterV3 {
        struct ExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint24 fee;
            address recipient;
            uint256 deadline;
            uint256 amountIn;
            uint256 amountOutMinimum;
            uint160 sqrtPriceLimitX96;
        }

        function exactInputSingle(ExactInputSingleParams calldata params)
            external
            payable
            returns (uint256 amountOut);
    }
}

/// TickMath bounds (matching Uniswap V3 SDK and dex_calldata.rs).
pub const MIN_SQRT_RATIO_PLUS_ONE: U160 = U160::from_limbs([4_295_128_740, 0, 0]);
pub const MAX_SQRT_RATIO_MINUS_ONE: U160 = U160::from_limbs([
    6_743_328_256_752_651_557,
    17_280_870_778_742_802_505,
    4_294_805_859,
]);

/// Resolve the wrapped native token address for a chain ID.
pub fn resolve_wrapped_native(chain_id: u64) -> Result<Address, String> {
    if let Some(addr) = vaughan_core::core::wpls_for_chain(chain_id) {
        return Ok(addr);
    }
    match chain_id {
        1 | 31337 => "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
            .parse()
            .map_err(|e| format!("{e}")),
        11155111 => "0x7b79995e5f793A07Bc00c21412e50Ecae098E7f9"
            .parse()
            .map_err(|e| format!("{e}")),
        other => Err(format!("no wrapped native configured for chain {other}")),
    }
}

fn default_deadline() -> U256 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    U256::from(now + 1200) // 20 minutes from now
}

fn evm_tx(
    chain_id: u64,
    from: Address,
    to: Address,
    value: U256,
    calldata: Vec<u8>,
) -> EvmTransaction {
    EvmTransaction {
        from: format!("{from:#x}"),
        to: format!("{to:#x}"),
        value: value.to_string(),
        data: if calldata.is_empty() {
            None
        } else {
            Some(format!("0x{}", hex::encode(calldata)))
        },
        gas_limit: None,
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id,
    }
}

/// Build an EVM transaction for native or ERC-20 transfer.
pub fn build_transfer_tx(
    chain_id: u64,
    from: Address,
    to: Address,
    token: Option<Address>,
    amount: U256,
) -> EvmTransaction {
    match token {
        None => evm_tx(chain_id, from, to, amount, vec![]),
        Some(token_addr) => {
            let calldata = IERC20Transfer::transferCall { to, amount }.abi_encode();
            evm_tx(chain_id, from, token_addr, U256::ZERO, calldata)
        }
    }
}

/// Build an EVM transaction for Uniswap V2 `swapExactTokensForTokens`.
pub fn build_v2_swap_tx(
    chain_id: u64,
    from: Address,
    router: Address,
    path: Vec<Address>,
    amount_in: U256,
    min_amount_out: U256,
    recipient: Address,
) -> EvmTransaction {
    let deadline = default_deadline();
    let calldata = IUniswapV2RouterSwap::swapExactTokensForTokensCall {
        amountIn: amount_in,
        amountOutMin: min_amount_out,
        path,
        to: recipient,
        deadline,
    }
    .abi_encode();

    evm_tx(chain_id, from, router, U256::ZERO, calldata)
}

/// Build an EVM transaction for Uniswap V3 `exactInputSingle`.
///
/// Built from the re-derived quote: `amountOutMinimum` equals `min_amount_out`
/// (which was checked to satisfy `min_amount_out >= quote * (1 - max_slippage_bps)`).
#[allow(clippy::too_many_arguments)]
pub fn build_v3_swap_tx(
    chain_id: u64,
    from: Address,
    router: Address,
    token_in: Address,
    token_out: Address,
    fee: u32,
    amount_in: U256,
    min_amount_out: U256,
    recipient: Address,
) -> EvmTransaction {
    let deadline = default_deadline();
    let sqrt_price_limit = if token_in < token_out {
        MIN_SQRT_RATIO_PLUS_ONE
    } else {
        MAX_SQRT_RATIO_MINUS_ONE
    };
    let fee_u24 = U24::try_from(fee).unwrap_or(U24::from(3000));
    let params = ISwapRouterV3::ExactInputSingleParams {
        tokenIn: token_in,
        tokenOut: token_out,
        fee: fee_u24,
        recipient,
        deadline,
        amountIn: amount_in,
        amountOutMinimum: min_amount_out,
        sqrtPriceLimitX96: sqrt_price_limit,
    };
    let calldata = ISwapRouterV3::exactInputSingleCall { params }.abi_encode();

    evm_tx(chain_id, from, router, U256::ZERO, calldata)
}

/// Build an EVM transaction for ERC-20 `approve(spender, amount)`.
pub fn build_approve_tx(
    chain_id: u64,
    from: Address,
    token: Address,
    spender: Address,
    amount: U256,
) -> EvmTransaction {
    let calldata = IERC20Approve::approveCall { spender, amount }.abi_encode();
    evm_tx(chain_id, from, token, U256::ZERO, calldata)
}

/// Build an EVM transaction for WETH9 wrap (`deposit()`).
pub fn build_wrap_tx(chain_id: u64, from: Address, amount: U256) -> Result<EvmTransaction, String> {
    let weth = resolve_wrapped_native(chain_id)?;
    let calldata = IWETH9::depositCall {}.abi_encode();
    Ok(evm_tx(chain_id, from, weth, amount, calldata))
}

/// Build an EVM transaction for WETH9 unwrap (`withdraw(wad)`).
pub fn build_unwrap_tx(
    chain_id: u64,
    from: Address,
    amount: U256,
) -> Result<EvmTransaction, String> {
    let weth = resolve_wrapped_native(chain_id)?;
    let calldata = IWETH9::withdrawCall { wad: amount }.abi_encode();
    Ok(evm_tx(chain_id, from, weth, U256::ZERO, calldata))
}

/// Build an EIP-7702 batched transfer transaction using Ambire `execute(txns)`.
pub fn build_batch_transfer_tx(
    chain_id: u64,
    from: Address,
    legs: &[BatchTransferLeg],
) -> EvmTransaction {
    let total_value = legs
        .iter()
        .fold(U256::ZERO, |acc, l| acc.saturating_add(l.amount));
    let txns: Vec<vaughan_aa::Transaction> = legs
        .iter()
        .map(|l| vaughan_aa::Transaction {
            to: l.to,
            value: l.amount,
            data: alloy::primitives::Bytes::new(),
        })
        .collect();
    let calldata = vaughan_aa::encode_execute(&txns, &[]);

    evm_tx(chain_id, from, from, total_value, calldata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vaughan_core::core::decode_v3_swap_calldata;

    #[test]
    fn test_v3_calldata_amount_out_matches_min_out() {
        let from = Address::repeat_byte(0x99);
        let router = Address::repeat_byte(0x11);
        let token_in = Address::repeat_byte(0x22);
        let token_out = Address::repeat_byte(0x33);
        let recipient = Address::repeat_byte(0x44);
        let amount_in = U256::from(100_000);
        let min_amount_out = U256::from(99_000);

        let tx = build_v3_swap_tx(
            369,
            from,
            router,
            token_in,
            token_out,
            3000,
            amount_in,
            min_amount_out,
            recipient,
        );

        assert_eq!(tx.to, format!("{router:#x}"));
        assert_eq!(tx.value, "0");

        let hex_data = tx.data.unwrap();
        let calldata_bytes = hex::decode(hex_data.trim_start_matches("0x")).unwrap();
        let decoded = decode_v3_swap_calldata(&calldata_bytes).unwrap();
        assert_eq!(decoded.recipient, recipient);
        assert_eq!(decoded.amount_in, amount_in);
        assert_eq!(decoded.amount_out_min, min_amount_out);
        assert_eq!(decoded.path, vec![token_in, token_out]);
    }

    #[test]
    fn test_batch_transfer_calldata_decodable() {
        let from = Address::repeat_byte(0x77);
        let recipient1 = Address::repeat_byte(0x55);
        let recipient2 = Address::repeat_byte(0x66);
        let legs = vec![
            BatchTransferLeg {
                to: recipient1,
                amount: U256::from(10),
            },
            BatchTransferLeg {
                to: recipient2,
                amount: U256::from(20),
            },
        ];

        let tx = build_batch_transfer_tx(369, from, &legs);
        assert_eq!(tx.to, format!("{from:#x}"));
        assert_eq!(tx.value, "30");

        let hex_data = tx.data.unwrap();
        let calldata_bytes = hex::decode(hex_data.trim_start_matches("0x")).unwrap();
        let decoded_txns = vaughan_aa::decode_execute(&calldata_bytes).unwrap();
        assert_eq!(decoded_txns.len(), 2);
        assert_eq!(decoded_txns[0].to, recipient1);
        assert_eq!(decoded_txns[0].value, U256::from(10));
        assert_eq!(decoded_txns[1].to, recipient2);
        assert_eq!(decoded_txns[1].value, U256::from(20));
    }
}
