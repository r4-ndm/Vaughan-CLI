//! Protocol and contract capability fingerprinting.
//!
//! Probes contract interfaces on-chain via non-reverting static `eth_call`s
//! to automatically detect standard protocol interfaces (ERC-20, Uniswap V2/V3,
//! Multicall3, WETH, etc.).

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::rpc::types::eth::TransactionRequest;
use serde::{Deserialize, Serialize};

/// Standard interface fingerprints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "details")]
pub enum ContractFingerprint {
    /// Standard ERC-20 fungible token.
    Erc20 {
        name: Option<String>,
        symbol: Option<String>,
        decimals: Option<u8>,
    },
    /// Uniswap V2 or PulseX Factory.
    UniswapV2Factory { all_pairs_length: Option<u64> },
    /// Uniswap V2 or PulseX Liquidity Pair.
    UniswapV2Pair {
        token0: Address,
        token1: Address,
        reserve0: U256,
        reserve1: U256,
    },
    /// Uniswap V3 Factory.
    UniswapV3Factory,
    /// Uniswap V3 Liquidity Pool.
    UniswapV3Pool {
        token0: Address,
        token1: Address,
        fee: u32,
        sqrt_price_x96: Option<U256>,
        tick: Option<i32>,
        liquidity: Option<u128>,
    },
    /// Wrapped Native Asset (WETH / WPLS).
    Weth,
    /// Multicall3 aggregation contract.
    Multicall3,
    /// Generic or unidentified contract.
    Generic {
        has_code: bool,
        matched_capabilities: Vec<String>,
    },
}

impl ContractFingerprint {
    /// Calculate V2 spot price (token1 per token0) normalized for decimals.
    pub fn v2_spot_price(&self, decimals_token0: u8, decimals_token1: u8) -> Option<f64> {
        if let ContractFingerprint::UniswapV2Pair { reserve0, reserve1, .. } = self {
            let r0 = reserve0.to::<u128>() as f64 / 10f64.powi(decimals_token0 as i32);
            let r1 = reserve1.to::<u128>() as f64 / 10f64.powi(decimals_token1 as i32);
            if r0 > 0.0 {
                return Some(r1 / r0);
            }
        }
        None
    }

    /// Calculate V3 spot price (token1 per token0) from sqrtPriceX96 normalized for decimals.
    pub fn v3_spot_price(&self, decimals_token0: u8, decimals_token1: u8) -> Option<f64> {
        if let ContractFingerprint::UniswapV3Pool { sqrt_price_x96: Some(sqrt), .. } = self {
            let sqrt_val = sqrt.to::<u128>() as f64 / 2f64.powi(96);
            let raw_p = sqrt_val * sqrt_val;
            let dec_adj = 10f64.powi(decimals_token0 as i32 - decimals_token1 as i32);
            return Some(raw_p * dec_adj);
        }
        None
    }
}

/// Selector constants for probe checks.
mod selectors {
    pub const DECIMALS: [u8; 4] = [0x31, 0x3c, 0xe7, 0xf2]; // decimals()
    pub const SYMBOL: [u8; 4] = [0x95, 0xd8, 0x9b, 0x41]; // symbol()
    pub const NAME: [u8; 4] = [0x06, 0xfd, 0xde, 0x03]; // name()
    pub const TOTAL_SUPPLY: [u8; 4] = [0x18, 0x16, 0x0d, 0xdd]; // totalSupply()
    pub const GET_RESERVES: [u8; 4] = [0x09, 0x02, 0xf1, 0xac]; // getReserves()
    pub const TOKEN0: [u8; 4] = [0x0d, 0xfe, 0x16, 0x81]; // token0()
    pub const TOKEN1: [u8; 4] = [0xd2, 0x12, 0x20, 0xa7]; // token1()
    pub const ALL_PAIRS_LENGTH: [u8; 4] = [0x57, 0x4f, 0x2b, 0xa3]; // allPairsLength()
    pub const SLOT0: [u8; 4] = [0x38, 0x50, 0xc7, 0xbd]; // slot0()
    pub const FEE: [u8; 4] = [0xdd, 0xca, 0x3f, 0x43]; // fee()
    pub const LIQUIDITY: [u8; 4] = [0x1a, 0x68, 0x65, 0x02]; // liquidity()
    pub const TRY_AGGREGATE: [u8; 4] = [0xb1, 0xa3, 0x20, 0x3d]; // tryAggregate(bool,Call[])
    pub const DEPOSIT: [u8; 4] = [0xd0, 0xe3, 0x0d, 0xb0]; // deposit()
    pub const WITHDRAW: [u8; 4] = [0x2e, 0x1a, 0x7d, 0x4d]; // withdraw(uint256)
}

/// Capability Prober.
pub struct ContractProber;

impl ContractProber {
    /// Probe and fingerprint a target address.
    pub async fn probe<P: Provider>(provider: &P, target: Address) -> ContractFingerprint {
        let code = provider.get_code_at(target).await.unwrap_or_default();
        if code.is_empty() {
            return ContractFingerprint::Generic {
                has_code: false,
                matched_capabilities: vec![],
            };
        }

        // 1. Probe for Uniswap V2 / PulseX Pair (token0 + token1 + getReserves)
        if let (Some(t0), Some(t1), Some((r0, r1))) = (
            probe_address(provider, target, selectors::TOKEN0).await,
            probe_address(provider, target, selectors::TOKEN1).await,
            probe_v2_reserves(provider, target).await,
        ) {
            return ContractFingerprint::UniswapV2Pair {
                token0: t0,
                token1: t1,
                reserve0: r0,
                reserve1: r1,
            };
        }

        // 2. Probe for Uniswap V2 / PulseX Factory (allPairsLength)
        if let Some(len) = probe_u64(provider, target, selectors::ALL_PAIRS_LENGTH).await {
            return ContractFingerprint::UniswapV2Factory {
                all_pairs_length: Some(len),
            };
        }

        // 3. Probe for Uniswap V3 Pool (slot0 + token0 + fee)
        if let (Some(t0), Some(t1), Some(fee)) = (
            probe_address(provider, target, selectors::TOKEN0).await,
            probe_address(provider, target, selectors::TOKEN1).await,
            probe_u32(provider, target, selectors::FEE).await,
        ) {
            let slot0_data = probe_slot0(provider, target).await;
            if slot0_data.is_some() || probe_call(provider, target, selectors::SLOT0).await.is_some() {
                let (sqrt_price_x96, tick) = slot0_data.unwrap_or((None, None));
                let liquidity = probe_u128(provider, target, selectors::LIQUIDITY).await;
                return ContractFingerprint::UniswapV3Pool {
                    token0: t0,
                    token1: t1,
                    fee,
                    sqrt_price_x96,
                    tick,
                    liquidity,
                };
            }
        }

        // 4. Probe for Multicall3 (tryAggregate)
        if probe_selector_success(provider, target, selectors::TRY_AGGREGATE).await {
            return ContractFingerprint::Multicall3;
        }

        // 5. Probe for WETH / Wrapped Native (deposit + withdraw + ERC20)
        let has_deposit = probe_selector_success(provider, target, selectors::DEPOSIT).await;
        let has_withdraw = probe_selector_success(provider, target, selectors::WITHDRAW).await;

        // 6. Probe for ERC-20 Token (symbol + decimals + totalSupply)
        let has_total_supply = probe_u256(provider, target, selectors::TOTAL_SUPPLY)
            .await
            .is_some();
        let sym = probe_string(provider, target, selectors::SYMBOL).await;
        let dec = probe_u8(provider, target, selectors::DECIMALS).await;
        let name = probe_string(provider, target, selectors::NAME).await;

        if has_deposit && has_withdraw {
            return ContractFingerprint::Weth;
        }

        if has_total_supply || sym.is_some() || dec.is_some() {
            return ContractFingerprint::Erc20 {
                name,
                symbol: sym,
                decimals: dec,
            };
        }

        ContractFingerprint::Generic {
            has_code: true,
            matched_capabilities: vec![],
        }
    }
}

async fn probe_call<P: Provider>(
    provider: &P,
    target: Address,
    selector: [u8; 4],
) -> Option<Bytes> {
    let tx = TransactionRequest::default()
        .to(target)
        .input(Bytes::copy_from_slice(&selector).into());

    provider.call(tx).await.ok()
}

async fn probe_selector_success<P: Provider>(
    provider: &P,
    target: Address,
    selector: [u8; 4],
) -> bool {
    let tx = TransactionRequest::default()
        .to(target)
        .input(Bytes::copy_from_slice(&selector).into());

    // Some contracts like multicall might revert on empty params or return data
    provider.call(tx).await.is_ok()
}

async fn probe_address<P: Provider>(
    provider: &P,
    target: Address,
    selector: [u8; 4],
) -> Option<Address> {
    let out = probe_call(provider, target, selector).await?;
    if out.len() >= 32 {
        let addr = Address::from_slice(&out[12..32]);
        if !addr.is_zero() {
            return Some(addr);
        }
    }
    None
}

async fn probe_string<P: Provider>(
    provider: &P,
    target: Address,
    selector: [u8; 4],
) -> Option<String> {
    let out = probe_call(provider, target, selector).await?;
    if out.is_empty() {
        return None;
    }

    // Attempt standard ABI string decode (offset + len + data)
    if out.len() >= 64 {
        let len = U256::from_be_slice(&out[32..64]).to::<usize>();
        if 64 + len <= out.len() {
            if let Ok(s) = std::str::from_utf8(&out[64..64 + len]) {
                return Some(s.to_string());
            }
        }
    }

    // Direct bytes32 string fallback
    if out.len() == 32 {
        let clean: Vec<u8> = out.iter().copied().take_while(|&b| b != 0).collect();
        if let Ok(s) = std::str::from_utf8(&clean) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }

    None
}

async fn probe_u8<P: Provider>(provider: &P, target: Address, selector: [u8; 4]) -> Option<u8> {
    let out = probe_call(provider, target, selector).await?;
    if out.len() >= 32 {
        Some(out[31])
    } else {
        None
    }
}

async fn probe_u32<P: Provider>(provider: &P, target: Address, selector: [u8; 4]) -> Option<u32> {
    let out = probe_call(provider, target, selector).await?;
    if out.len() >= 32 {
        Some(U256::from_be_slice(&out[..32]).to::<u32>())
    } else {
        None
    }
}

async fn probe_u64<P: Provider>(provider: &P, target: Address, selector: [u8; 4]) -> Option<u64> {
    let out = probe_call(provider, target, selector).await?;
    if out.len() >= 32 {
        Some(U256::from_be_slice(&out[..32]).to::<u64>())
    } else {
        None
    }
}

async fn probe_u256<P: Provider>(provider: &P, target: Address, selector: [u8; 4]) -> Option<U256> {
    let out = probe_call(provider, target, selector).await?;
    if out.len() >= 32 {
        Some(U256::from_be_slice(&out[..32]))
    } else {
        None
    }
}

async fn probe_v2_reserves<P: Provider>(provider: &P, target: Address) -> Option<(U256, U256)> {
    let out = probe_call(provider, target, selectors::GET_RESERVES).await?;
    if out.len() >= 64 {
        let r0 = U256::from_be_slice(&out[..32]);
        let r1 = U256::from_be_slice(&out[32..64]);
        Some((r0, r1))
    } else {
        None
    }
}

async fn probe_u128<P: Provider>(provider: &P, target: Address, selector: [u8; 4]) -> Option<u128> {
    let out = probe_call(provider, target, selector).await?;
    if out.len() >= 32 {
        Some(U256::from_be_slice(&out[..32]).to::<u128>())
    } else {
        None
    }
}

async fn probe_slot0<P: Provider>(provider: &P, target: Address) -> Option<(Option<U256>, Option<i32>)> {
    let out = probe_call(provider, target, selectors::SLOT0).await?;
    if out.len() >= 64 {
        let sqrt = U256::from_be_slice(&out[..32]);
        let tick = i32::from_be_bytes(out[60..64].try_into().unwrap_or_default());
        Some((Some(sqrt), Some(tick)))
    } else {
        None
    }
}
