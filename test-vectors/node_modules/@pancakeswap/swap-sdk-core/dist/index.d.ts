type BigintIsh = bigint | number | string;
declare enum TradeType {
    EXACT_INPUT = 0,
    EXACT_OUTPUT = 1
}
declare enum Rounding {
    ROUND_DOWN = 0,
    ROUND_HALF_UP = 1,
    ROUND_UP = 2
}
declare const MINIMUM_LIQUIDITY = 1000n;
declare const ZERO = 0n;
declare const ONE = 1n;
declare const TWO = 2n;
declare const THREE = 3n;
declare const FIVE = 5n;
declare const TEN = 10n;
declare const _100 = 100n;
declare const _9975 = 9975n;
declare const _10000 = 10000n;
declare const MaxUint256: bigint;
declare enum VMType {
    uint8 = "uint8",
    uint256 = "uint256"
}
declare const VM_TYPE_MAXIMA: {
    uint8: bigint;
    uint256: bigint;
};
declare const ZERO_ADDRESS: "0x0000000000000000000000000000000000000000";

/**
 * A currency is any fungible financial instrument, including Ether, all ERC20 tokens, and other chain-native currencies
 */
declare abstract class BaseCurrency<T extends BaseCurrency<any> = BaseCurrency<any>> {
    /**
     * Returns whether the currency is native to the chain and must be wrapped (e.g. Ether)
     */
    abstract readonly isNative: boolean;
    /**
     * Returns whether the currency is a token that is usable in PancakeSwap without wrapping
     */
    abstract readonly isToken: boolean;
    /**
     * The chain ID on which this currency resides
     */
    readonly chainId: number;
    /**
     * The decimals used in representing currency amounts
     */
    readonly decimals: number;
    /**
     * The symbol of the currency, i.e. a short textual non-unique identifier
     */
    readonly symbol: string;
    /**
     * The name of the currency, i.e. a descriptive textual non-unique identifier
     */
    readonly name?: string;
    /**
     * Constructs an instance of the base class `BaseCurrency`.
     * @param chainId the chain ID on which this currency resides
     * @param decimals decimals of the currency
     * @param symbol symbol of the currency
     * @param name of the currency
     */
    protected constructor(chainId: number, decimals: number, symbol: string, name?: string);
    /**
     * Returns whether this currency is functionally equivalent to the other currency
     * @param other the other currency
     */
    abstract equals(other: BaseCurrency<any>): boolean;
    /**
     * Return the wrapped version of this currency that can be used with the PancakeSwap contracts. Currencies must
     * implement this to be used in PancakeSwap
     */
    abstract get wrapped(): T;
    get asToken(): T;
}

interface SerializedToken {
    chainId: number;
    address: `0x${string}`;
    decimals: number;
    symbol: string;
    name?: string;
    projectLink?: string;
}
/**
 * Represents an ERC20 token with a unique address and some metadata.
 */
declare class Token extends BaseCurrency<Token> {
    readonly isNative: false;
    readonly isToken: true;
    /**
     * The contract address on the chain on which this token lives
     */
    readonly address: `0x${string}`;
    readonly projectLink?: string;
    constructor(chainId: number, address: `0x${string}`, decimals: number, symbol: string, name?: string, projectLink?: string);
    /**
     * Returns true if the two tokens are equivalent, i.e. have the same chainId and address.
     * @param other other token to compare
     */
    equals(other: BaseCurrency): boolean;
    /**
     * Returns true if the address of this token sorts before the address of the other token
     * @param other other token to compare
     * @throws if the tokens have the same address
     * @throws if the tokens are on different chains
     */
    sortsBefore(other: Token): boolean;
    /**
     * Return this token, which does not need to be wrapped
     */
    get wrapped(): Token;
    get serialize(): SerializedToken;
}

/**
 * Represents the native currency of the chain on which it resides, e.g.
 */
declare abstract class NativeCurrency extends BaseCurrency<Token> {
    readonly isNative: true;
    readonly isToken: false;
    get asToken(): Token;
}

interface SerializedSPLToken {
    chainId: number;
    address: string;
    programId: string;
    decimals: number;
    symbol: string;
    name?: string;
    projectLink?: string;
}
/**
 * Represents an SPL token on Solana or other non-EVM chains.
 */
declare class SPLToken extends BaseCurrency<SPLToken> {
    readonly isNative: false;
    readonly isToken: true;
    readonly address: string;
    readonly programId: string;
    readonly logoURI: string;
    readonly projectLink?: string;
    static isSPLToken(token?: UnifiedCurrency): boolean;
    constructor({ chainId, programId, address, decimals, symbol, logoURI, name, projectLink, }: {
        chainId: number;
        programId: string;
        address: string;
        decimals: number;
        symbol: string;
        logoURI: string;
        name?: string;
        projectLink?: string;
        isNative?: boolean;
    });
    /**
     * Returns true if the two tokens are equivalent, i.e. have the same chainId and programId.
     * @param other other token to compare
     */
    equals(other: BaseCurrency): boolean;
    sortsBefore(other: SPLToken): boolean;
    get wrapped(): SPLToken;
    get serialize(): SerializedSPLToken;
}

/**
 * Represents the native currency of the chain on which it resides, e.g.
 */
declare abstract class SPLNativeCurrency extends BaseCurrency<SPLToken> {
    readonly isNative: true;
    readonly isToken: false;
    readonly address: string;
}

type Currency = NativeCurrency | Token;
type UnifiedNativeCurrency = NativeCurrency | SPLNativeCurrency;
type UnifiedCurrency = SPLToken | SPLNativeCurrency | Currency;
type UnifiedToken = SPLToken | Token;

declare class Fraction {
    readonly numerator: bigint;
    readonly denominator: bigint;
    constructor(numerator: BigintIsh, denominator?: BigintIsh);
    private static tryParseFraction;
    get quotient(): bigint;
    get remainder(): Fraction;
    invert(): Fraction;
    add(other: Fraction | BigintIsh): Fraction;
    subtract(other: Fraction | BigintIsh): Fraction;
    lessThan(other: Fraction | BigintIsh): boolean;
    equalTo(other: Fraction | BigintIsh): boolean;
    greaterThan(other: Fraction | BigintIsh): boolean;
    multiply(other: Fraction | BigintIsh): Fraction;
    divide(other: Fraction | BigintIsh): Fraction;
    toSignificant(significantDigits: number, format?: object, rounding?: Rounding): string;
    toFixed(decimalPlaces: number, format?: object, rounding?: Rounding): string;
    /**
     * Helper method for converting any super class back to a fraction
     */
    get asFraction(): Fraction;
}

/**
 * Converts a fraction to a percent
 * @param fraction the fraction to convert
 */
declare function toPercent(fraction: Fraction): Percent;
declare class Percent extends Fraction {
    /**
     * This boolean prevents a fraction from being interpreted as a Percent
     */
    readonly isPercent: true;
    static toPercent: typeof toPercent;
    add(other: Fraction | BigintIsh): Percent;
    subtract(other: Fraction | BigintIsh): Percent;
    multiply(other: Fraction | BigintIsh): Percent;
    divide(other: Fraction | BigintIsh): Percent;
    toSignificant(significantDigits?: number, format?: object, rounding?: Rounding): string;
    toFixed(decimalPlaces?: number, format?: object, rounding?: Rounding): string;
}

declare class CurrencyAmount<T extends Currency> extends Fraction {
    readonly currency: T;
    readonly decimalScale: bigint;
    /**
     * Returns a new currency amount instance from the unitless amount of token, i.e. the raw amount
     * @param currency the currency in the amount
     * @param rawAmount the raw token or ether amount
     */
    static fromRawAmount<T extends Currency>(currency: T, rawAmount: BigintIsh): CurrencyAmount<T>;
    /**
     * Construct a currency amount with a denominator that is not equal to 1
     * @param currency the currency
     * @param numerator the numerator of the fractional token amount
     * @param denominator the denominator of the fractional token amount
     */
    static fromFractionalAmount<T extends Currency>(currency: T, numerator: BigintIsh, denominator: BigintIsh): CurrencyAmount<T>;
    protected constructor(currency: T, numerator: BigintIsh, denominator?: BigintIsh);
    add(other: CurrencyAmount<T>): CurrencyAmount<T>;
    subtract(value: bigint): CurrencyAmount<T>;
    subtract(other: CurrencyAmount<T>): CurrencyAmount<T>;
    multiply(other: Fraction | BigintIsh): CurrencyAmount<T>;
    divide(other: Fraction | BigintIsh): CurrencyAmount<T>;
    toSignificant(significantDigits?: number, format?: object, rounding?: Rounding): string;
    toFixed(decimalPlaces?: number, format?: object, rounding?: Rounding): string;
    toExact(format?: object): string;
    get wrapped(): CurrencyAmount<Token>;
    info(): string;
}

declare class UnifiedCurrencyAmount<T extends UnifiedCurrency> extends Fraction {
    readonly currency: T;
    readonly decimalScale: bigint;
    /**
     * Returns a new currency amount instance from the unitless amount of token, i.e. the raw amount
     * @param currency the currency in the amount
     * @param rawAmount the raw token or ether amount
     */
    static fromRawAmount<T extends UnifiedCurrency>(currency: T, rawAmount: BigintIsh): UnifiedCurrencyAmount<T>;
    /**
     * Construct a currency amount with a denominator that is not equal to 1
     * @param currency the currency
     * @param numerator the numerator of the fractional token amount
     * @param denominator the denominator of the fractional token amount
     */
    static fromFractionalAmount<T extends UnifiedCurrency>(currency: T, numerator: BigintIsh, denominator: BigintIsh): UnifiedCurrencyAmount<T>;
    protected constructor(currency: T, numerator: BigintIsh, denominator?: BigintIsh);
    add(other: UnifiedCurrencyAmount<T>): UnifiedCurrencyAmount<T>;
    subtract(value: bigint): UnifiedCurrencyAmount<T>;
    subtract(other: UnifiedCurrencyAmount<T>): UnifiedCurrencyAmount<T>;
    multiply(other: Fraction | BigintIsh): UnifiedCurrencyAmount<T>;
    divide(other: Fraction | BigintIsh): UnifiedCurrencyAmount<T>;
    toSignificant(significantDigits?: number, format?: object, rounding?: Rounding): string;
    toFixed(decimalPlaces?: number, format?: object, rounding?: Rounding): string;
    toExact(format?: object): string;
    get wrapped(): UnifiedCurrencyAmount<UnifiedToken>;
    info(): string;
}

declare class Price<TBase extends UnifiedCurrency, TQuote extends UnifiedCurrency> extends Fraction {
    readonly baseCurrency: TBase;
    readonly quoteCurrency: TQuote;
    readonly scalar: Fraction;
    /**
     * Construct a price, either with the base and quote currency amount, or the
     * @param args
     */
    constructor(...args: [TBase, TQuote, BigintIsh, BigintIsh] | [{
        baseAmount: UnifiedCurrencyAmount<TBase>;
        quoteAmount: UnifiedCurrencyAmount<TQuote>;
    }]);
    /**
     * Flip the price, switching the base and quote currency
     */
    invert(): Price<TQuote, TBase>;
    /**
     * Multiply the price by another price, returning a new price. The other price must have the same base currency as this price's quote currency
     * @param other the other price
     */
    multiply<TOtherQuote extends UnifiedCurrency>(other: Price<TQuote, TOtherQuote>): Price<TBase, TOtherQuote>;
    /**
     * Return the amount of quote currency corresponding to a given amount of the base currency
     * @param currencyAmount the amount of base currency to quote against the price
     */
    quote(currencyAmount: UnifiedCurrencyAmount<TBase>): UnifiedCurrencyAmount<TQuote>;
    /**
     * Get the value scaled by decimals for formatting
     * @private
     */
    private get adjustedForDecimals();
    toSignificant(significantDigits?: number, format?: object, rounding?: Rounding): string;
    toFixed(decimalPlaces?: number, format?: object, rounding?: Rounding): string;
    get wrapped(): Price<TBase['wrapped'], TQuote['wrapped']>;
    /**
     * Create a price from base and quote currency and a decimal string
     * @param base
     * @param quote
     * @param value
     * @returns Price<TBase, TQuote> | undefined
     */
    static fromDecimal<TBase extends UnifiedCurrency, TQuote extends UnifiedCurrency>(base: TBase, quote: TQuote, value: string): Price<TBase, TQuote> | undefined;
}

/**
 * Indicates that the pair has insufficient reserves for a desired output amount. I.e. the amount of output cannot be
 * obtained by sending any amount of input.
 */
declare class InsufficientReservesError extends Error {
    readonly isInsufficientReservesError: true;
    constructor();
}
/**
 * Indicates that the input amount is too small to produce any amount of output. I.e. the amount of input sent is less
 * than the price of a single unit of output after fees.
 */
declare class InsufficientInputAmountError extends Error {
    readonly isInsufficientInputAmountError: true;
    constructor();
}

declare function validateVMTypeInstance(value: bigint, vmType: VMType): void;
declare function sqrt(y: bigint): bigint;
declare function sortedInsert<T>(items: T[], add: T, maxSize: number, comparator: (a: T, b: T) => number): T | null;
/**
 * Returns the percent difference between the mid price and the execution price, i.e. price impact.
 * @param midPrice mid price before the trade
 * @param inputAmount the input amount of the trade
 * @param outputAmount the output amount of the trade
 */
declare function computePriceImpact<TBase extends Currency, TQuote extends Currency>(midPrice: Price<TBase, TQuote>, inputAmount: CurrencyAmount<TBase>, outputAmount: CurrencyAmount<TQuote>): Percent;
declare function getTokenComparator(balances: {
    [tokenAddress: string]: CurrencyAmount<Token> | undefined;
}): (tokenA: Token, tokenB: Token) => number;
declare function sortCurrencies<T extends Currency>(currencies: T[]): T[];
declare function sortUnifiedCurrencies<T extends UnifiedCurrency>(currencies: T[]): T[];
declare const isCurrencySorted: (currencyA: Currency, currencyB: Currency) => boolean;
declare const isUnifiedCurrencySorted: (currencyA: UnifiedCurrency, currencyB: UnifiedCurrency) => boolean;
declare function getCurrencyAddress(currency: Currency): `0x${string}`;
declare function getUnifiedCurrencyAddress(currency: UnifiedCurrency): string;
declare function getMatchedCurrency(currency: Currency, list: Currency[], matchWrappedCurrency?: boolean): Currency | undefined;

export { BaseCurrency, BigintIsh, Currency, CurrencyAmount, FIVE, Fraction, InsufficientInputAmountError, InsufficientReservesError, MINIMUM_LIQUIDITY, MaxUint256, NativeCurrency, ONE, Percent, Price, Rounding, SPLNativeCurrency, SPLToken, SerializedSPLToken, SerializedToken, TEN, THREE, TWO, Token, TradeType, UnifiedCurrency, UnifiedCurrencyAmount, UnifiedNativeCurrency, UnifiedToken, VMType, VM_TYPE_MAXIMA, ZERO, ZERO_ADDRESS, _100, _10000, _9975, computePriceImpact, getCurrencyAddress, getMatchedCurrency, getTokenComparator, getUnifiedCurrencyAddress, isCurrencySorted, isUnifiedCurrencySorted, sortCurrencies, sortUnifiedCurrencies, sortedInsert, sqrt, validateVMTypeInstance };
