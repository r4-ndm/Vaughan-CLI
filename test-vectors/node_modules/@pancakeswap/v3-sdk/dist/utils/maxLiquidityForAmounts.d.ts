import { BigintIsh } from '@pancakeswap/sdk';
/**
 * Returns an imprecise maximum amount of liquidity received for a given amount of token 0.
 * This function is available to accommodate LiquidityAmounts#getLiquidityForAmount0 in the v3 periphery,
 * which could be more precise by at least 32 bits by dividing by Q64 instead of Q96 in the intermediate step,
 * and shifting the subtracted ratio left by 32 bits. This imprecise calculation will likely be replaced in a future
 * v3 router contract.
 * @param sqrtRatioAX96 The price at the lower boundary
 * @param sqrtRatioBX96 The price at the upper boundary
 * @param amount0 The token0 amount
 * @returns liquidity for amount0, imprecise
 */
export declare function maxLiquidityForAmount0Imprecise(sqrtRatioAX96: bigint, sqrtRatioBX96: bigint, amount0: BigintIsh): bigint;
/**
 * Returns a precise maximum amount of liquidity received for a given amount of token 0 by dividing by Q64 instead of Q96 in the intermediate step,
 * and shifting the subtracted ratio left by 32 bits.
 * @param sqrtRatioAX96 The price at the lower boundary
 * @param sqrtRatioBX96 The price at the upper boundary
 * @param amount0 The token0 amount
 * @returns liquidity for amount0, precise
 */
export declare function maxLiquidityForAmount0Precise(sqrtRatioAX96: bigint, sqrtRatioBX96: bigint, amount0: BigintIsh): bigint;
/**
 * Computes the maximum amount of liquidity received for a given amount of token1
 * @param sqrtRatioAX96 The price at the lower tick boundary
 * @param sqrtRatioBX96 The price at the upper tick boundary
 * @param amount1 The token1 amount
 * @returns liquidity for amount1
 */
export declare function maxLiquidityForAmount1(sqrtRatioAX96: bigint, sqrtRatioBX96: bigint, amount1: BigintIsh): bigint;
/**
 * Computes the maximum amount of liquidity received for a given amount of token0, token1,
 * and the prices at the tick boundaries.
 * @param sqrtRatioCurrentX96 the current price
 * @param sqrtRatioAX96 price at lower boundary
 * @param sqrtRatioBX96 price at upper boundary
 * @param amount0 token0 amount
 * @param amount1 token1 amount
 * @param useFullPrecision if false, liquidity will be maximized according to what the router can calculate,
 * not what core can theoretically support
 */
export declare function maxLiquidityForAmounts(sqrtRatioCurrentX96: bigint, sqrtRatioAX96: bigint, sqrtRatioBX96: bigint, amount0: BigintIsh, amount1: BigintIsh, useFullPrecision: boolean): bigint;
//# sourceMappingURL=maxLiquidityForAmounts.d.ts.map