//! Alloy `sol!` ABI bindings for the PancakeSwap V3 contracts.
//!
//! Generated from `pancakeswap/pancake-v3-contracts` at commit
//! `986847948755cba528324d41be19480731c36c2a` (the pinned fork source).
//! Interface bodies are embedded verbatim (imports/comments stripped) so the
//! generated selectors and encodings match the on-chain ABIs exactly.
//!
//! Contracts covered:
//! - `PancakeV3Factory` (core)
//! - `PancakeV3Pool` (core)
//! - `SwapRouter` (periphery)
//! - `NonfungiblePositionManager` (periphery)

alloy::sol! {
    // SPDX-License-Identifier: GPL-2.0-or-later
    pragma solidity >=0.5.0;

    interface IPancakeV3Factory {
        struct TickSpacingExtraInfo {
            bool whitelistRequested;
            bool enabled;
        }

        event OwnerChanged(address indexed oldOwner, address indexed newOwner);
        event PoolCreated(
            address indexed token0,
            address indexed token1,
            uint24 indexed fee,
            int24 tickSpacing,
            address pool
        );
        event FeeAmountEnabled(uint24 indexed fee, int24 indexed tickSpacing);
        event FeeAmountExtraInfoUpdated(uint24 indexed fee, bool whitelistRequested, bool enabled);
        event WhiteListAdded(address indexed user, bool verified);
        event SetLmPoolDeployer(address indexed lmPoolDeployer);

        function owner() external view returns (address);
        function feeAmountTickSpacing(uint24 fee) external view returns (int24);
        function feeAmountTickSpacingExtraInfo(uint24 fee) external view returns (bool whitelistRequested, bool enabled);
        function getPool(
            address tokenA,
            address tokenB,
            uint24 fee
        ) external view returns (address pool);
        function createPool(
            address tokenA,
            address tokenB,
            uint24 fee
        ) external returns (address pool);
        function setOwner(address _owner) external;
        function enableFeeAmount(uint24 fee, int24 tickSpacing) external;
        function setWhiteListAddress(address user, bool verified) external;
        function setFeeAmountExtraInfo(
            uint24 fee,
            bool whitelistRequested,
            bool enabled
        ) external;
        function setLmPoolDeployer(address _lmPoolDeployer) external;
        function setFeeProtocol(address pool, uint32 feeProtocol0, uint32 feeProtocol1) external;
        function collectProtocol(
            address pool,
            address recipient,
            uint128 amount0Requested,
            uint128 amount1Requested
        ) external returns (uint128 amount0, uint128 amount1);
        function setLmPool(address pool, address lmPool) external;
    }

    interface IPancakeV3Pool {
        function factory() external view returns (address);
        function token0() external view returns (address);
        function token1() external view returns (address);
        function fee() external view returns (uint24);
        function tickSpacing() external view returns (int24);
        function maxLiquidityPerTick() external view returns (uint128);

        function slot0()
            external
            view
            returns (
                uint160 sqrtPriceX96,
                int24 tick,
                uint16 observationIndex,
                uint16 observationCardinality,
                uint16 observationCardinalityNext,
                uint32 feeProtocol,
                bool unlocked
            );
        function feeGrowthGlobal0X128() external view returns (uint256);
        function feeGrowthGlobal1X128() external view returns (uint256);
        function protocolFees() external view returns (uint128 token0, uint128 token1);
        function liquidity() external view returns (uint128);
        function ticks(int24 tick)
            external
            view
            returns (
                uint128 liquidityGross,
                int128 liquidityNet,
                uint256 feeGrowthOutside0X128,
                uint256 feeGrowthOutside1X128,
                int56 tickCumulativeOutside,
                uint160 secondsPerLiquidityOutsideX128,
                uint32 secondsOutside,
                bool initialized
            );
        function tickBitmap(int16 wordPosition) external view returns (uint256);
        function positions(bytes32 key)
            external
            view
            returns (
                uint128 _liquidity,
                uint256 feeGrowthInside0LastX128,
                uint256 feeGrowthInside1LastX128,
                uint128 tokensOwed0,
                uint128 tokensOwed1
            );

        function initialize(uint160 sqrtPriceX96) external;
        function mint(
            address recipient,
            int24 tickLower,
            int24 tickUpper,
            uint128 amount,
            bytes calldata data
        ) external returns (uint256 amount0, uint256 amount1);
        function collect(
            address recipient,
            int24 tickLower,
            int24 tickUpper,
            uint128 amount0Requested,
            uint128 amount1Requested
        ) external returns (uint128 amount0, uint128 amount1);
        function burn(
            int24 tickLower,
            int24 tickUpper,
            uint128 amount
        ) external returns (uint256 amount0, uint256 amount1);
        function swap(
            address recipient,
            bool zeroForOne,
            int256 amountSpecified,
            uint160 sqrtPriceLimitX96,
            bytes calldata data
        ) external returns (int256 amount0, int256 amount1);
        function flash(
            address recipient,
            uint256 amount0,
            uint256 amount1,
            bytes calldata data
        ) external;
        function increaseObservationCardinalityNext(uint16 observationCardinalityNext) external;
        function observe(uint32[] calldata secondsAgos)
            external
            view
            returns (
                int56[] memory tickCumulatives,
                uint160[] memory secondsPerLiquidityCumulativeX128s
            );
        function snapshotCumulativesInside(int24 tickLower, int24 tickUpper)
            external
            view
            returns (
                int56 tickCumulativeInside,
                uint160 secondsPerLiquidityInsideX128,
                uint32 secondsInside
            );
    }

    interface IPancakeV3SwapCallback {
        function pancakeV3SwapCallback(
            int256 amount0Delta,
            int256 amount1Delta,
            bytes calldata data
        ) external;
    }

    interface ISwapRouter is IPancakeV3SwapCallback {
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

        struct ExactOutputSingleParams {
            address tokenIn;
            address tokenOut;
            uint24 fee;
            address recipient;
            uint256 deadline;
            uint256 amountOut;
            uint256 amountInMaximum;
            uint160 sqrtPriceLimitX96;
        }

        struct ExactOutputParams {
            bytes path;
            address recipient;
            uint256 deadline;
            uint256 amountOut;
            uint256 amountInMaximum;
        }

        function exactInputSingle(ExactInputSingleParams calldata params) external payable returns (uint256 amountOut);
        function exactInput(ExactInputParams calldata params) external payable returns (uint256 amountOut);
        function exactOutputSingle(ExactOutputSingleParams calldata params) external payable returns (uint256 amountIn);
        function exactOutput(ExactOutputParams calldata params) external payable returns (uint256 amountIn);
    }

    interface INonfungiblePositionManager {
        struct MintParams {
            address token0;
            address token1;
            uint24 fee;
            int24 tickLower;
            int24 tickUpper;
            uint256 amount0Desired;
            uint256 amount1Desired;
            uint256 amount0Min;
            uint256 amount1Min;
            address recipient;
            uint256 deadline;
        }

        struct IncreaseLiquidityParams {
            uint256 tokenId;
            uint256 amount0Desired;
            uint256 amount1Desired;
            uint256 amount0Min;
            uint256 amount1Min;
            uint256 deadline;
        }

        struct DecreaseLiquidityParams {
            uint256 tokenId;
            uint128 liquidity;
            uint256 amount0Min;
            uint256 amount1Min;
            uint256 deadline;
        }

        struct CollectParams {
            uint256 tokenId;
            address recipient;
            uint128 amount0Max;
            uint128 amount1Max;
        }

        event IncreaseLiquidity(uint256 indexed tokenId, uint128 liquidity, uint256 amount0, uint256 amount1);
        event DecreaseLiquidity(uint256 indexed tokenId, uint128 liquidity, uint256 amount0, uint256 amount1);
        event Collect(uint256 indexed tokenId, address recipient, uint256 amount0, uint256 amount1);

        function deployer() external view returns (address);
        function factory() external view returns (address);
        function WETH9() external view returns (address);

        function positions(uint256 tokenId)
            external
            view
            returns (
                uint96 nonce,
                address operator,
                address token0,
                address token1,
                uint24 fee,
                int24 tickLower,
                int24 tickUpper,
                uint128 liquidity,
                uint256 feeGrowthInside0LastX128,
                uint256 feeGrowthInside1LastX128,
                uint128 tokensOwed0,
                uint128 tokensOwed1
            );

        function mint(MintParams calldata params)
            external
            payable
            returns (
                uint256 tokenId,
                uint128 liquidity,
                uint256 amount0,
                uint256 amount1
            );
        function increaseLiquidity(IncreaseLiquidityParams calldata params)
            external
            payable
            returns (
                uint128 liquidity,
                uint256 amount0,
                uint256 amount1
            );
        function decreaseLiquidity(DecreaseLiquidityParams calldata params)
            external
            payable
            returns (uint256 amount0, uint256 amount1);
        function collect(CollectParams calldata params) external payable returns (uint256 amount0, uint256 amount1);
        function burn(uint256 tokenId) external payable;
    }

    /// Minimal ERC20 surface needed by the SDK: allowance checks, approvals,
    /// decimals, balance reads, and transfers.
    interface IERC20Minimal {
        function allowance(address owner, address spender) external view returns (uint256);
        function approve(address spender, uint256 amount) external returns (bool);
        function decimals() external view returns (uint8);
        function balanceOf(address account) external view returns (uint256);
        function transfer(address to, uint256 value) external returns (bool);
    }

    /// Minimal ERC721 surface needed for position ownership checks.
    interface IERC721Minimal {
        event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);
        function ownerOf(uint256 tokenId) external view returns (address);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;
    use alloy::sol_types::SolCall;

    /// The bindings must reproduce PancakeSwap's exact function selectors.
    /// Expected values verified via `cast sig` against the fork ABI.
    #[test]
    fn factory_selectors_match_pancakeswap() {
        // getPool(address,address,uint24) -> 0x1698ee82
        assert_eq!(
            IPancakeV3Factory::getPoolCall::SELECTOR,
            [0x16, 0x98, 0xee, 0x82],
            "getPool selector"
        );
        // createPool(address,address,uint24) -> 0xa1671295
        assert_eq!(
            IPancakeV3Factory::createPoolCall::SELECTOR,
            [0xa1, 0x67, 0x12, 0x95],
            "createPool selector"
        );
        // owner() -> 0x8da5cb5b
        assert_eq!(
            IPancakeV3Factory::ownerCall::SELECTOR,
            [0x8d, 0xa5, 0xcb, 0x5b],
            "owner selector"
        );
    }

    #[test]
    fn pool_selectors_match_pancakeswap() {
        // slot0() -> 0x3850c7bd
        assert_eq!(
            IPancakeV3Pool::slot0Call::SELECTOR,
            [0x38, 0x50, 0xc7, 0xbd],
            "slot0 selector"
        );
        // swap(address,bool,int256,uint160,bytes) -> 0x128acb08
        assert_eq!(
            IPancakeV3Pool::swapCall::SELECTOR,
            [0x12, 0x8a, 0xcb, 0x08],
            "swap selector"
        );
    }

    #[test]
    fn router_selectors_match_pancakeswap() {
        // exactInputSingle((address,address,uint24,address,uint256,uint256,uint256,uint160))
        // -> 0x414bf389
        assert_eq!(
            ISwapRouter::exactInputSingleCall::SELECTOR,
            [0x41, 0x4b, 0xf3, 0x89],
            "exactInputSingle selector"
        );
    }

    #[test]
    fn npm_selectors_match_pancakeswap() {
        // positions(uint256) -> 0x99fbab88
        assert_eq!(
            INonfungiblePositionManager::positionsCall::SELECTOR,
            [0x99, 0xfb, 0xab, 0x88],
            "positions selector"
        );
        // mint((address,address,uint24,int24,int24,uint256,uint256,uint256,uint256,address,uint256))
        // -> 0x88316456
        assert_eq!(
            INonfungiblePositionManager::mintCall::SELECTOR,
            [0x88, 0x31, 0x64, 0x56],
            "mint selector"
        );
    }

    /// A MintParams must be constructible with the ABI types (used by Phase 2
    /// tx builders). Note: fee is `U24`, ticks are `I24` (alloy aliases), and
    /// amounts are `U256`.
    #[test]
    fn npm_mint_params_struct_is_usable() {
        use alloy::primitives::{aliases::I24, aliases::U24, U256};

        let params = INonfungiblePositionManager::MintParams {
            token0: address!("1111111111111111111111111111111111111111"),
            token1: address!("2222222222222222222222222222222222222222"),
            fee: U24::try_from(500u32).unwrap(),
            tickLower: I24::try_from(-600i32).unwrap(),
            tickUpper: I24::try_from(600i32).unwrap(),
            amount0Desired: U256::from(1_000_000_000u64),
            amount1Desired: U256::from(2_000_000_000u64),
            amount0Min: U256::ZERO,
            amount1Min: U256::ZERO,
            recipient: address!("3333333333333333333333333333333333333333"),
            deadline: U256::from(0xffff_ffffu64),
        };
        assert_eq!(params.fee, U24::try_from(500u32).unwrap());
        assert_eq!(params.tickLower, I24::try_from(-600i32).unwrap());
    }
}
