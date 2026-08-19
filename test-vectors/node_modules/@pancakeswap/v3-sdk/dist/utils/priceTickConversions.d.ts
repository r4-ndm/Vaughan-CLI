import { Currency, Price, Token } from '@pancakeswap/swap-sdk-core';
/**
 * Returns a price object corresponding to the input tick and the base/quote token
 * Inputs must be tokens because the address order is used to interpret the price represented by the tick
 * @param baseToken the base token of the price
 * @param quoteToken the quote token of the price
 * @param tick the tick for which to return the price
 */
export declare function tickToPrice<TBase extends Currency | Token, TQuote extends Currency | Token>(baseToken: TBase, quoteToken: TQuote, tick: number): Price<TBase, TQuote>;
/**
 * Returns a price object corresponding to the input tick and the base/quote token
 * Inputs must be tokens because the address order is used to interpret the price represented by the tick
 * @param baseToken the base token of the price
 * @param quoteToken the quote token of the price
 * @param tick the tick for which to return the price
 */
export declare function tickToPriceV2(baseToken: Currency, quoteToken: Currency, tick: number): Price<Currency, Currency>;
/**
 * Returns the first tick for which the given price is greater than or equal to the tick price
 * @param price for which to return the closest tick that represents a price less than or equal to the input price,
 * i.e. the price of the returned tick is less than or equal to the input price
 */
export declare function priceToClosestTick(price: Price<Currency, Currency>): number;
//# sourceMappingURL=priceTickConversions.d.ts.map