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

    interface IERC20Allowance {
        function allowance(address owner, address spender) external view returns (uint256);
    }

    /// WETH9-shaped wrap (Pulse WPLS uses the same selectors).
    interface IWETH9 {
        function deposit() external payable;
        function withdraw(uint256 wad) external;
        function balanceOf(address account) external view returns (uint256);
    }
}

/// V2 vs Uniswap-V3-style SwapRouter periphery.
pub use vaughan_core::core::DexProtocol;

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
    /// V3 fee tier (ignored for V2); first hop when [`hop_fees`] is set.
    pub fee: u32,
    /// Per-hop V3 fee tiers from quote discovery (`path.len() - 1`).
    pub hop_fees: Option<Vec<u32>>,
    pub recipient: Address,
    pub from: String,
    pub chain_id: u64,
    /// When set (V3 quote path), overrides [`hop_tokens`].
    pub hops: Option<Vec<Address>>,
}

/// TickMath bounds — matches `wiz4rd-sdk` swap router defaults (not `U160::ZERO`).
const MIN_SQRT_RATIO_PLUS_ONE: U160 = U160::from_limbs([4_295_128_740, 0, 0]);
const MAX_SQRT_RATIO_MINUS_ONE: U160 = U160::from_limbs([
    6_743_328_256_752_651_557,
    17_280_870_778_742_802_505,
    4_294_805_859,
]);

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
    let hop_fees = vec![fee; tokens.len().saturating_sub(1)];
    vaughan_core::core::encode_v3_packed_path(tokens, &hop_fees).map_err(|e| e.user_message())
}

pub fn encode_v3_path_hops(tokens: &[Address], hop_fees: &[u32]) -> Result<Bytes, String> {
    vaughan_core::core::encode_v3_packed_path(tokens, hop_fees).map_err(|e| e.user_message())
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

/// ERC-20 revoke = `approve(spender, 0)`.
pub fn build_revoke_tx(
    token: Address,
    spender: Address,
    from: &str,
    chain_id: u64,
) -> EvmTransaction {
    build_approve_tx(token, spender, U256::ZERO, from, chain_id)
}

/// Wrap native PLS/ETH: `deposit()` payable on a WETH9-shaped contract.
pub fn build_wrap_tx(wpls: Address, amount_wei: U256, from: &str, chain_id: u64) -> EvmTransaction {
    let data = Bytes::from(IWETH9::depositCall {}.abi_encode());
    EvmTransaction {
        from: from.to_string(),
        to: format!("{wpls:#x}"),
        value: amount_wei.to_string(),
        data: Some(format!("0x{}", hex::encode(data.as_ref()))),
        gas_limit: None,
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id,
    }
}

/// Unwrap WPLS: `withdraw(wad)`.
pub fn build_unwrap_tx(
    wpls: Address,
    amount_wei: U256,
    from: &str,
    chain_id: u64,
) -> EvmTransaction {
    let data = Bytes::from(IWETH9::withdrawCall { wad: amount_wei }.abi_encode());
    EvmTransaction {
        from: from.to_string(),
        to: format!("{wpls:#x}"),
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

/// ABI calldata for `allowance(owner, spender)` (eth_call).
pub fn encode_allowance_call(owner: Address, spender: Address) -> String {
    let data = Bytes::from(IERC20Allowance::allowanceCall { owner, spender }.abi_encode());
    format!("0x{}", hex::encode(data.as_ref()))
}

/// ABI calldata for `balanceOf(account)` (eth_call).
pub fn encode_balance_of_call(account: Address) -> String {
    let data = Bytes::from(IWETH9::balanceOfCall { account }.abi_encode());
    format!("0x{}", hex::encode(data.as_ref()))
}

/// Build a V2 or V3 swap transaction (no signing).
pub fn build_swap_tx(req: &DexSwapRequest) -> Result<EvmTransaction, String> {
    let hops = match &req.hops {
        Some(path) => path.clone(),
        None => hop_tokens(req.token_in, req.token_out, req.wpls, req.native_in)?,
    };
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
        DexProtocol::V3 => encode_v3_swap(req, &hops, deadline)?,
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
    req: &DexSwapRequest,
    hops: &[Address],
    deadline: U256,
) -> Result<(U256, Bytes), String> {
    let value = if req.native_in {
        req.amount_in
    } else {
        U256::ZERO
    };
    let data = if hops.len() == 2 {
        let hop_fee = req
            .hop_fees
            .as_deref()
            .and_then(|f| f.first().copied())
            .unwrap_or(req.fee);
        let fee = U24::try_from(hop_fee).map_err(|e| format!("bad fee: {e}"))?;
        // token0 < token1 in Uniswap V3 pools — token_in < token_out ⇒ zeroForOne.
        let sqrt_price_limit = if hops[0] < hops[1] {
            MIN_SQRT_RATIO_PLUS_ONE
        } else {
            MAX_SQRT_RATIO_MINUS_ONE
        };
        let params = ISwapRouterV3::ExactInputSingleParams {
            tokenIn: hops[0],
            tokenOut: hops[1],
            fee,
            recipient: req.recipient,
            deadline,
            amountIn: req.amount_in,
            amountOutMinimum: req.min_out,
            sqrtPriceLimitX96: sqrt_price_limit,
        };
        Bytes::from(ISwapRouterV3::exactInputSingleCall { params }.abi_encode())
    } else {
        let fees: Vec<u32> = req
            .hop_fees
            .clone()
            .unwrap_or_else(|| vec![req.fee; hops.len() - 1]);
        let path = encode_v3_path_hops(hops, &fees)?;
        let params = ISwapRouterV3::ExactInputParams {
            path,
            recipient: req.recipient,
            deadline,
            amountIn: req.amount_in,
            amountOutMinimum: req.min_out,
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

pub fn weth_deposit_selector() -> [u8; 4] {
    IWETH9::depositCall::SELECTOR
}

pub fn weth_withdraw_selector() -> [u8; 4] {
    IWETH9::withdrawCall::SELECTOR
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
    fn v3_path_packing_per_hop_fees() {
        let a = Address::repeat_byte(0x11);
        let w = Address::repeat_byte(0xaa);
        let b = Address::repeat_byte(0x22);
        let packed = encode_v3_path_hops(&[a, w, b], &[500, 20_000]).unwrap();
        assert_eq!(packed.len(), 66);
        assert_eq!(&packed[20..23], &[0, 0x01, 0xf4]);
        assert_eq!(&packed[43..46], &[0, 0x4e, 0x20]);
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
            hop_fees: None,
            recipient: Address::repeat_byte(0x33),
            from: format!("{:#x}", Address::repeat_byte(0x33)),
            chain_id: 943,
            hops: None,
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
            hop_fees: None,
            recipient: Address::repeat_byte(0x33),
            from: format!("{:#x}", Address::repeat_byte(0x33)),
            chain_id: 943,
            hops: None,
        };
        let tx = build_swap_tx(&req).unwrap();
        let data = hex::decode(tx.data.as_ref().unwrap().trim_start_matches("0x")).unwrap();
        assert_eq!(&data[..4], &v3_exact_input_single_selector());
    }
}
