//! EmpX / EmpSeal on-chain aggregator (PulseChain) — Alloy interop only.
//!
//! Interface facts from the public `empx-swap-sdk` (router address +
//! `findBestPath` / `swapNoSplit*` shapes). No TS SDK vendored.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::sol;
use alloy::sol_types::SolCall;
use url::Url;

use super::catalog::AggVenue;
use super::routers::assert_agg_exec_targets;
use super::types::{AggExecTx, AggQuote, AggQuoteRequest};
use crate::error::WalletError;

/// EmpX router on PulseChain mainnet (from public EmpX SDK chain config).
pub const EMPX_ROUTER_369: &str = "0x0Cf6D948Cf09ac83a6bf40C7AD7b44657A9F2A52";

/// WPLS — EmpX resolves native → wrapped for path finding.
const WPLS_369: &str = "0xA1077a294dDE1B09bB078844df40758a5D0f9a27";

/// Protocol fee bps applied by EmpX swaps (SDK default).
const PROTOCOL_FEE_BPS: u64 = 28;

sol! {
    #[derive(Debug)]
    struct Trade {
        uint256 amountIn;
        uint256 amountOut;
        address[] path;
        address[] adapters;
    }

    interface IEmpxRouter {
        function findBestPath(
            uint256 amountIn,
            address tokenIn,
            address tokenOut,
            uint256 maxSteps
        ) external view returns (
            uint256[] amounts,
            address[] path,
            address[] adapters,
            uint256 gasEstimate
        );

        function swapNoSplit(Trade trade, address to, uint256 feeBps) external;
        function swapNoSplitFromPLS(Trade trade, address to, uint256 feeBps) external payable;
        function swapNoSplitToPLS(Trade trade, address to, uint256 feeBps) external;
    }
}

/// Quote + calldata via EmpX on-chain router (chain 369).
pub struct EmpxClient {
    rpc_url: String,
}

impl EmpxClient {
    pub fn for_chain(chain_id: u64, rpc_url: &str) -> Result<Self, WalletError> {
        if chain_id != 369 && chain_id != 31337 {
            return Err(WalletError::Other(
                "EmpX Alloy client is wired for PulseChain mainnet (369) only".into(),
            ));
        }
        Ok(Self {
            rpc_url: rpc_url.to_string(),
        })
    }

    pub async fn quote(&self, req: &AggQuoteRequest) -> Result<AggQuote, WalletError> {
        let router: Address = EMPX_ROUTER_369
            .parse()
            .map_err(|e| WalletError::Other(format!("EmpX router: {e}")))?;
        let wpls: Address = WPLS_369
            .parse()
            .map_err(|e| WalletError::Other(format!("WPLS: {e}")))?;

        let token_in = if req.token_in_is_native {
            wpls
        } else {
            req.token_in
        };
        let token_out = if req.token_out_is_native {
            wpls
        } else {
            req.token_out
        };

        let url = Url::parse(&self.rpc_url)
            .map_err(|e| WalletError::Other(format!("invalid EmpX RPC URL: {e}")))?;
        let provider: alloy::providers::RootProvider<alloy::network::Ethereum> =
            alloy::providers::RootProvider::new_http(url);

        let call = IEmpxRouter::findBestPathCall {
            amountIn: req.amount_in,
            tokenIn: token_in,
            tokenOut: token_out,
            maxSteps: U256::from(3u64),
        };
        let raw = provider
            .call(
                alloy::rpc::types::eth::TransactionRequest::default()
                    .to(router)
                    .input(Bytes::from(call.abi_encode()).into()),
            )
            .await
            .map_err(|e| WalletError::RpcError(format!("EmpX findBestPath: {e}")))?;
        let decoded = IEmpxRouter::findBestPathCall::abi_decode_returns(&raw)
            .map_err(|e| WalletError::Other(format!("EmpX path decode: {e}")))?;

        if decoded.path.len() < 2 || decoded.amounts.is_empty() {
            return Err(WalletError::Other("EmpX: empty path".into()));
        }
        let amount_out_raw = *decoded.amounts.last().unwrap();
        let slippage_bps = ((req.slippage_percent * 100.0).round() as u64).min(5_000);
        let min_out = amount_out_raw
            .saturating_mul(U256::from(10_000u64.saturating_sub(slippage_bps)))
            / U256::from(10_000u64);

        let trade = Trade {
            amountIn: req.amount_in,
            amountOut: min_out,
            path: decoded.path.clone(),
            adapters: decoded.adapters.clone(),
        };

        let recipient = req.account.ok_or_else(|| {
            WalletError::Other("EmpX quote needs account (recipient) address".into())
        })?;
        let fee = U256::from(PROTOCOL_FEE_BPS);

        let (data, value) = if req.token_in_is_native {
            (
                Bytes::from(
                    IEmpxRouter::swapNoSplitFromPLSCall {
                        trade,
                        to: recipient,
                        feeBps: fee,
                    }
                    .abi_encode(),
                ),
                req.amount_in,
            )
        } else if req.token_out_is_native {
            (
                Bytes::from(
                    IEmpxRouter::swapNoSplitToPLSCall {
                        trade,
                        to: recipient,
                        feeBps: fee,
                    }
                    .abi_encode(),
                ),
                U256::ZERO,
            )
        } else {
            (
                Bytes::from(
                    IEmpxRouter::swapNoSplitCall {
                        trade,
                        to: recipient,
                        feeBps: fee,
                    }
                    .abi_encode(),
                ),
                U256::ZERO,
            )
        };

        assert_agg_exec_targets(router, router)?;

        Ok(AggQuote {
            venue: AggVenue::Empseal,
            amount_in: req.amount_in,
            amount_out: amount_out_raw,
            gas_estimate: Some({
                let g = decoded.gasEstimate;
                if g > U256::from(u64::MAX) {
                    250_000
                } else {
                    g.to::<u64>().max(250_000)
                }
            }),
            tx: AggExecTx {
                to: router,
                data,
                value,
            },
            spender: router,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn router_parses() {
        let a: Address = EMPX_ROUTER_369.parse().unwrap();
        assert_eq!(a, address!("0x0Cf6D948Cf09ac83a6bf40C7AD7b44657A9F2A52"));
    }
}
