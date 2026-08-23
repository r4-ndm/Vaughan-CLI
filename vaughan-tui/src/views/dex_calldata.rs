//! Calldata builders for PulseChain V2 / V3 DEX swaps.
//!
//! Shared by [`super::DexView`] and Anvil integration tests so UI and on-chain
//! paths stay in lockstep.

use alloy::primitives::aliases::{U160, U24};
use alloy::primitives::{Address, Bytes, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use vaughan_core::chains::EvmTransaction;

sol! {
    interface IUniswapV2RouterSwap {
        function swapExactETHForTokens(
            uint256 amountOutMin,
            address[] calldata path,
            address to,
            uint256 deadline
        ) external payable returns (uint256[] memory amounts);

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

        struct ExactInputParams {
            bytes path;
            address recipient;
            uint256 deadline;
            uint256 amountIn;
            uint256 amountOutMinimum;
        }

        function exactInputSingle(ExactInputSingleParams calldata params)
            external
            payable
            returns (uint256 amountOut);

        function exactInput(ExactInputParams calldata params)
            external
            payable
            returns (uint256 amountOut);
    }

    interface IERC20Approve {
        function approve(address spender, uint256 amount) external returns (bool);
    }
}

/// V2 vs Uniswap-V3-style SwapRouter periphery.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DexProtocol {
    V2,
    V3,
}

/// Parameters for [`build_swap_tx`].
#[derive(Clone, Debug)]
pub struct DexSwapRequest {
    pub protocol: DexProtocol,
    pub router: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub wpls: Option<Address>,
    pub native_in: bool,
    pub amount_in: U256,
    pub min_out: U256,
    /// V3 fee tier (ignored for V2).
    pub fee: u32,
    pub recipient: Address,
    pub from: String,
    pub chain_id: u64,
}

/// Token hop list: native→token is WPLS→out; meme/meme is in→WPLS→out.
pub fn hop_tokens(
    token_in: Address,
    token_out: Address,
    wpls: Option<Address>,
    native_in: bool,
) -> Result<Vec<Address>, String> {
    if native_in {
        let wpls = wpls.ok_or_else(|| {
            "native→token needs WPLS (PulseChain) — set network or paste WPLS as token in"
                .to_string()
        })?;
        if token_out == wpls {
            return Err("token out cannot be WPLS for native→token".into());
        }
        return Ok(vec![wpls, token_out]);
    }
    if token_in == token_out {
        return Err("token in and token out must differ".into());
    }
    match wpls {
        Some(w) if token_in != w && token_out != w => Ok(vec![token_in, w, token_out]),
        _ => Ok(vec![token_in, token_out]),
    }
}

/// Uniswap V3 packed path: `token (20) || fee (3) || token (20) || …`.
pub fn encode_v3_path(tokens: &[Address], fee: u32) -> Result<Bytes, String> {
    if tokens.len() < 2 {
        return Err("V3 path needs ≥2 tokens".into());
    }
    if fee > 0xFF_FFFF {
        return Err("fee tier out of uint24 range".into());
    }
    let mut out = Vec::with_capacity(tokens.len() * 20 + (tokens.len() - 1) * 3);
    for (i, token) in tokens.iter().enumerate() {
        out.extend_from_slice(token.as_slice());
        if i + 1 < tokens.len() {
            out.push(((fee >> 16) & 0xff) as u8);
            out.push(((fee >> 8) & 0xff) as u8);
            out.push((fee & 0xff) as u8);
        }
    }
    Ok(Bytes::from(out))
}

/// ERC-20 `approve(router, amount)` against `token_in`.
pub fn build_approve_tx(
    token_in: Address,
    router: Address,
    amount: U256,
    from: &str,
    chain_id: u64,
) -> EvmTransaction {
    let call = IERC20Approve::approveCall {
        spender: router,
        amount,
    };
    let data = Bytes::from(call.abi_encode());
    EvmTransaction {
        from: from.to_string(),
        to: format!("{token_in:#x}"),
        value: "0".into(),
        data: Some(format!("0x{}", hex::encode(data.as_ref()))),
        gas_limit: None,
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id,
    }
}

/// Build a V2 or V3 swap transaction (no signing).
pub fn build_swap_tx(req: &DexSwapRequest) -> Result<EvmTransaction, String> {
    let hops = hop_tokens(req.token_in, req.token_out, req.wpls, req.native_in)?;
    let deadline = U256::from(u64::MAX);
    let (value, data) = match req.protocol {
        DexProtocol::V2 => encode_v2_swap(
            &hops,
            req.amount_in,
            req.min_out,
            req.recipient,
            deadline,
            req.native_in,
        )?,
        DexProtocol::V3 => encode_v3_swap(
            &hops,
            req.amount_in,
            req.min_out,
            req.recipient,
            deadline,
            req.fee,
            req.native_in,
        )?,
    };
    Ok(EvmTransaction {
        from: req.from.clone(),
        to: format!("{:#x}", req.router),
        value: value.to_string(),
        data: Some(format!("0x{}", hex::encode(data.as_ref()))),
        gas_limit: None,
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id: req.chain_id,
    })
}

fn encode_v2_swap(
    hops: &[Address],
    amount_in: U256,
    min_out: U256,
    recipient: Address,
    deadline: U256,
    native_in: bool,
) -> Result<(U256, Bytes), String> {
    let path = hops.to_vec();
    if native_in {
        let call = IUniswapV2RouterSwap::swapExactETHForTokensCall {
            amountOutMin: min_out,
            path,
            to: recipient,
            deadline,
        };
        Ok((amount_in, Bytes::from(call.abi_encode())))
    } else {
        let call = IUniswapV2RouterSwap::swapExactTokensForTokensCall {
            amountIn: amount_in,
            amountOutMin: min_out,
            path,
            to: recipient,
            deadline,
        };
        Ok((U256::ZERO, Bytes::from(call.abi_encode())))
    }
}

fn encode_v3_swap(
    hops: &[Address],
    amount_in: U256,
    min_out: U256,
    recipient: Address,
    deadline: U256,
    fee: u32,
    native_in: bool,
) -> Result<(U256, Bytes), String> {
    let value = if native_in { amount_in } else { U256::ZERO };
    let data = if hops.len() == 2 {
        let fee = U24::try_from(fee).map_err(|e| format!("bad fee: {e}"))?;
        let params = ISwapRouterV3::ExactInputSingleParams {
            tokenIn: hops[0],
            tokenOut: hops[1],
            fee,
            recipient,
            deadline,
            amountIn: amount_in,
            amountOutMinimum: min_out,
            sqrtPriceLimitX96: U160::ZERO,
        };
        Bytes::from(ISwapRouterV3::exactInputSingleCall { params }.abi_encode())
    } else {
        let path = encode_v3_path(hops, fee)?;
        let params = ISwapRouterV3::ExactInputParams {
            path,
            recipient,
            deadline,
            amountIn: amount_in,
            amountOutMinimum: min_out,
        };
        Bytes::from(ISwapRouterV3::exactInputCall { params }.abi_encode())
    };
    Ok((value, data))
}

/// Selectors used by Anvil mock routers (keep in sync with `sol!` above).
pub fn v2_swap_exact_eth_selector() -> [u8; 4] {
    IUniswapV2RouterSwap::swapExactETHForTokensCall::SELECTOR
}

pub fn v2_swap_exact_tokens_selector() -> [u8; 4] {
    IUniswapV2RouterSwap::swapExactTokensForTokensCall::SELECTOR
}

pub fn v3_exact_input_single_selector() -> [u8; 4] {
    ISwapRouterV3::exactInputSingleCall::SELECTOR
}

pub fn v3_exact_input_selector() -> [u8; 4] {
    ISwapRouterV3::exactInputCall::SELECTOR
}

pub fn erc20_approve_selector() -> [u8; 4] {
    IERC20Approve::approveCall::SELECTOR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meme_to_meme_hops_via_wpls() {
        let a = Address::repeat_byte(0x11);
        let b = Address::repeat_byte(0x22);
        let w = Address::repeat_byte(0xaa);
        assert_eq!(hop_tokens(a, b, Some(w), false).unwrap(), vec![a, w, b]);
    }

    #[test]
    fn v3_path_packing_two_hops() {
        let a = Address::repeat_byte(0x11);
        let w = Address::repeat_byte(0xaa);
        let b = Address::repeat_byte(0x22);
        let packed = encode_v3_path(&[a, w, b], 3000).unwrap();
        assert_eq!(packed.len(), 66);
        assert_eq!(&packed[20..23], &[0x00, 0x0b, 0xb8]);
    }

    #[test]
    fn v2_native_calldata_starts_with_selector() {
        let w = Address::repeat_byte(0xaa);
        let out = Address::repeat_byte(0x22);
        let req = DexSwapRequest {
            protocol: DexProtocol::V2,
            router: Address::repeat_byte(0x11),
            token_in: w,
            token_out: out,
            wpls: Some(w),
            native_in: true,
            amount_in: U256::from(1_000_000_000_000_000_000u64),
            min_out: U256::from(1u64),
            fee: 3000,
            recipient: Address::repeat_byte(0x33),
            from: format!("{:#x}", Address::repeat_byte(0x33)),
            chain_id: 943,
        };
        let tx = build_swap_tx(&req).unwrap();
        let data = hex::decode(tx.data.as_ref().unwrap().trim_start_matches("0x")).unwrap();
        assert_eq!(&data[..4], &v2_swap_exact_eth_selector());
        assert_eq!(tx.value, "1000000000000000000");
    }

    #[test]
    fn v3_single_calldata_starts_with_selector() {
        let w = Address::repeat_byte(0xaa);
        let out = Address::repeat_byte(0x22);
        let req = DexSwapRequest {
            protocol: DexProtocol::V3,
            router: Address::repeat_byte(0x11),
            token_in: w,
            token_out: out,
            wpls: Some(w),
            native_in: true,
            amount_in: U256::from(10u64),
            min_out: U256::from(1u64),
            fee: 3000,
            recipient: Address::repeat_byte(0x33),
            from: format!("{:#x}", Address::repeat_byte(0x33)),
            chain_id: 943,
        };
        let tx = build_swap_tx(&req).unwrap();
        let data = hex::decode(tx.data.as_ref().unwrap().trim_start_matches("0x")).unwrap();
        assert_eq!(&data[..4], &v3_exact_input_single_selector());
    }
}
