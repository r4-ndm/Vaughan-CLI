// Generates `vectors.json` — a corpus of exact-input → exact-output pairs for
// every math function wiz4rd-math re-exports, computed with the canonical
// TypeScript implementation (@pancakeswap/v3-sdk). The Rust tests in
// wiz4rd-math assert their output equals these values exactly.
//
// Note: this script uses CommonJS `require` on purpose. The package's ESM
// entry pulls in @pancakeswap/sdk -> swap-sdk-evm which imports a symbol that
// swap-sdk-core@1.6.0 does not export (upstream packaging bug); CJS resolves
// cleanly and the math utils are identical.
//
// Run:  npm install && npm run generate
// Output: vectors.json (committed; regenerate only when the SDK pin changes)

import { createRequire } from 'node:module';
import { writeFileSync } from 'node:fs';

const require = createRequire(import.meta.url);
const {
  TickMath,
  FullMath,
  SqrtPriceMath,
  LiquidityMath,
  SwapMath,
  PositionMath,
  FeeAmount,
} = require('@pancakeswap/v3-sdk');

const vectors = {
  // tick → sqrtPriceX96, and sqrtPriceX96 → tick
  tickToSqrtPrice: [
    TickMath.MIN_TICK,
    -100000, -10000, -500, -100, -1, 0, 1, 100, 500, 10000, 100000,
    TickMath.MAX_TICK,
  ].map((tick) => ({
    tick,
    sqrtPriceX96: TickMath.getSqrtRatioAtTick(tick).toString(),
  })),
  sqrtPriceToTick: [
    TickMath.MIN_SQRT_RATIO,
    79228162514264337593543950336n, // 2^96 (tick 0)
    TickMath.getSqrtRatioAtTick(500),
    TickMath.getSqrtRatioAtTick(-500),
    TickMath.getSqrtRatioAtTick(12345),
    TickMath.MAX_SQRT_RATIO - 1n, // MAX_SQRT_RATIO itself is exclusive
  ].map((sqrt) => ({
    sqrtPriceX96: sqrt.toString(),
    tick: TickMath.getTickAtSqrtRatio(sqrt),
  })),

  // mulDiv with rounding up (matches FullMath::mul_div_rounding_up)
  mulDivRoundingUp: [
    [1n, 1n, 1n],
    [7n, 3n, 2n],            // exact: 21/2 = 10.5 -> 11
    [100n, 100n, 3n],        // 10000/3 -> round up
    [2n ** 128n, 2n ** 128n, 2n ** 96n],
    [2n ** 128n, 2n ** 128n, 3n],        // large product, remainder -> rounds up
    [123456789n, 987654321n, 1000000007n],
    [10n ** 38n, 10n ** 38n, 3n],
    [0n, 5n, 7n],
  ].map(([a, b, d]) => ({
    a: a.toString(),
    b: b.toString(),
    denominator: d.toString(),
    result: FullMath.mulDivRoundingUp(a, b, d).toString(),
  })),

  // getNextSqrtPriceFromInput / Output (zeroForOne true/false)
  nextSqrtPriceFromInput: [
    { sqrtPX96: TickMath.getSqrtRatioAtTick(0), liquidity: 1000000000n, amountIn: 1000000n, zeroForOne: true },
    { sqrtPX96: TickMath.getSqrtRatioAtTick(0), liquidity: 1000000000n, amountIn: 1000000n, zeroForOne: false },
    { sqrtPX96: TickMath.getSqrtRatioAtTick(500), liquidity: 123456789n, amountIn: 999999n, zeroForOne: true },
    { sqrtPX96: TickMath.getSqrtRatioAtTick(-500), liquidity: 2n ** 100n, amountIn: 2n ** 60n, zeroForOne: false },
    { sqrtPX96: TickMath.getSqrtRatioAtTick(12345), liquidity: 1n, amountIn: 1n, zeroForOne: true },
    { sqrtPX96: TickMath.getSqrtRatioAtTick(0), liquidity: 10n ** 20n, amountIn: 10n ** 18n, zeroForOne: false },
  ].map((v) => ({
    ...v,
    sqrtPX96: v.sqrtPX96.toString(),
    liquidity: v.liquidity.toString(),
    amountIn: v.amountIn.toString(),
    result: SqrtPriceMath.getNextSqrtPriceFromInput(v.sqrtPX96, v.liquidity, v.amountIn, v.zeroForOne).toString(),
  })),
  nextSqrtPriceFromOutput: [
    { sqrtPX96: TickMath.getSqrtRatioAtTick(0), liquidity: 1000000000n, amountOut: 500000n, zeroForOne: true },
    { sqrtPX96: TickMath.getSqrtRatioAtTick(0), liquidity: 1000000000n, amountOut: 500000n, zeroForOne: false },
    { sqrtPX96: TickMath.getSqrtRatioAtTick(250), liquidity: 2n ** 90n, amountOut: 2n ** 50n, zeroForOne: true },
    { sqrtPX96: TickMath.getSqrtRatioAtTick(-250), liquidity: 10n ** 18n, amountOut: 10n ** 12n, zeroForOne: false },
  ].map((v) => ({
    ...v,
    sqrtPX96: v.sqrtPX96.toString(),
    liquidity: v.liquidity.toString(),
    amountOut: v.amountOut.toString(),
    result: SqrtPriceMath.getNextSqrtPriceFromOutput(v.sqrtPX96, v.liquidity, v.amountOut, v.zeroForOne).toString(),
  })),

  // amount deltas (roundUp true/false)
  amount0Delta: [
    { sqrtA: TickMath.getSqrtRatioAtTick(0), sqrtB: TickMath.getSqrtRatioAtTick(500), liquidity: 1000000n, roundUp: true },
    { sqrtA: TickMath.getSqrtRatioAtTick(0), sqrtB: TickMath.getSqrtRatioAtTick(500), liquidity: 1000000n, roundUp: false },
    { sqrtA: TickMath.getSqrtRatioAtTick(-500), sqrtB: TickMath.getSqrtRatioAtTick(500), liquidity: 2n ** 90n, roundUp: true },
    { sqrtA: TickMath.getSqrtRatioAtTick(100), sqrtB: TickMath.getSqrtRatioAtTick(200), liquidity: 10n ** 20n, roundUp: false },
  ].map((v) => ({
    ...v,
    sqrtA: v.sqrtA.toString(),
    sqrtB: v.sqrtB.toString(),
    liquidity: v.liquidity.toString(),
    result: SqrtPriceMath.getAmount0Delta(v.sqrtA, v.sqrtB, v.liquidity, v.roundUp).toString(),
  })),
  amount1Delta: [
    { sqrtA: TickMath.getSqrtRatioAtTick(0), sqrtB: TickMath.getSqrtRatioAtTick(500), liquidity: 1000000n, roundUp: true },
    { sqrtA: TickMath.getSqrtRatioAtTick(0), sqrtB: TickMath.getSqrtRatioAtTick(500), liquidity: 1000000n, roundUp: false },
    { sqrtA: TickMath.getSqrtRatioAtTick(-500), sqrtB: TickMath.getSqrtRatioAtTick(500), liquidity: 2n ** 90n, roundUp: true },
    { sqrtA: TickMath.getSqrtRatioAtTick(100), sqrtB: TickMath.getSqrtRatioAtTick(200), liquidity: 10n ** 20n, roundUp: false },
  ].map((v) => ({
    ...v,
    sqrtA: v.sqrtA.toString(),
    sqrtB: v.sqrtB.toString(),
    liquidity: v.liquidity.toString(),
    result: SqrtPriceMath.getAmount1Delta(v.sqrtA, v.sqrtB, v.liquidity, v.roundUp).toString(),
  })),

  // liquidity add/subtract
  addDelta: [
    { x: 100n, y: 50n },
    { x: 100n, y: -50n },
    { x: 0n, y: 12345n },
    { x: 2n ** 128n - 1n, y: 0n },
    { x: 10n ** 18n, y: -(10n ** 18n) },
  ].map((v) => ({
    x: v.x.toString(),
    y: v.y.toString(),
    result: LiquidityMath.addDelta(v.x, v.y).toString(),
  })),

  // swap step across fee tiers (exact in, exact out, partial)
  swapStep: [
    {
      sqrtCurrent: TickMath.getSqrtRatioAtTick(0),
      sqrtTarget: TickMath.getSqrtRatioAtTick(100),
      liquidity: 10n ** 18n,
      amountRemaining: 10n ** 15n,
      feePips: FeeAmount.LOWEST,
    },
    {
      sqrtCurrent: TickMath.getSqrtRatioAtTick(0),
      sqrtTarget: TickMath.getSqrtRatioAtTick(-100),
      liquidity: 10n ** 18n,
      amountRemaining: -(10n ** 15n),
      feePips: FeeAmount.MEDIUM,
    },
    {
      sqrtCurrent: TickMath.getSqrtRatioAtTick(1000),
      sqrtTarget: TickMath.getSqrtRatioAtTick(2000),
      liquidity: 2n ** 100n,
      amountRemaining: 2n ** 60n,
      feePips: FeeAmount.HIGH,
    },
    {
      sqrtCurrent: TickMath.getSqrtRatioAtTick(-1000),
      sqrtTarget: TickMath.getSqrtRatioAtTick(-2000),
      liquidity: 2n ** 100n,
      amountRemaining: -(2n ** 60n),
      feePips: FeeAmount.HIGH,
    },
    {
      sqrtCurrent: TickMath.getSqrtRatioAtTick(0),
      sqrtTarget: TickMath.getSqrtRatioAtTick(100),
      liquidity: 10n ** 18n,
      amountRemaining: 10n ** 30n, // > what target allows: reaches target
      feePips: FeeAmount.MEDIUM,
    },
  ].map((v) => {
    const [sqrtRatioNextX96, amountIn, amountOut, feeAmount] = SwapMath.computeSwapStep(
      v.sqrtCurrent, v.sqrtTarget, v.liquidity, v.amountRemaining, v.feePips,
    );
    return {
      sqrtCurrent: v.sqrtCurrent.toString(),
      sqrtTarget: v.sqrtTarget.toString(),
      liquidity: v.liquidity.toString(),
      amountRemaining: v.amountRemaining.toString(),
      feePips: v.feePips,
      sqrtRatioNextX96: sqrtRatioNextX96.toString(),
      amountIn: amountIn.toString(),
      amountOut: amountOut.toString(),
      feeAmount: feeAmount.toString(),
    };
  }),

  // position token amounts (in range / below / above)
  positionAmounts: [
    { tickCurrent: 0, tickLower: -100, tickUpper: 100, sqrtRatioX96: TickMath.getSqrtRatioAtTick(0), liquidity: 10n ** 18n },
    { tickCurrent: -200, tickLower: -100, tickUpper: 100, sqrtRatioX96: TickMath.getSqrtRatioAtTick(-200), liquidity: 10n ** 18n },
    { tickCurrent: 200, tickLower: -100, tickUpper: 100, sqrtRatioX96: TickMath.getSqrtRatioAtTick(200), liquidity: 10n ** 18n },
  ].map((v) => ({
    ...v,
    sqrtRatioX96: v.sqrtRatioX96.toString(),
    liquidity: v.liquidity.toString(),
    amount0: PositionMath.getToken0Amount(v.tickCurrent, v.tickLower, v.tickUpper, v.sqrtRatioX96, v.liquidity).toString(),
    amount1: PositionMath.getToken1Amount(v.tickCurrent, v.tickLower, v.tickUpper, v.sqrtRatioX96, v.liquidity).toString(),
  })),
};

writeFileSync(new URL('./vectors.json', import.meta.url), JSON.stringify(vectors, null, 2) + '\n');
console.log('wrote vectors.json with', Object.values(vectors).reduce((n, v) => n + v.length, 0), 'vectors');
