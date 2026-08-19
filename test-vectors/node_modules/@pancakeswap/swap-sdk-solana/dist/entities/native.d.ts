import { SPLNativeCurrency, SPLToken } from '@pancakeswap/swap-sdk-core';
/**
 *
 * Native is the main usage of a 'native' currency
 */
export declare class SPLNative extends SPLNativeCurrency {
    readonly programId: string;
    readonly address: string;
    readonly logoURI: string;
    constructor({ chainId, decimals, name, symbol, programId, address, logoURI, }: {
        chainId: number;
        decimals: number;
        symbol: string;
        name: string;
        programId: string;
        address: string;
        logoURI: string;
    });
    get wrapped(): SPLToken;
    equals(other: SPLToken | SPLNativeCurrency): boolean;
}
export declare const SOL: SPLNative;
//# sourceMappingURL=native.d.ts.map