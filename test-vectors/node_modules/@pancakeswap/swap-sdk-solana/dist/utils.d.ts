import { PublicKey } from '@solana/web3.js';
import { UnifiedCurrency } from '@pancakeswap/swap-sdk-core';
import { PublicKeyish } from './types';
export declare function tryParsePublicKey(v: string): PublicKey | string;
export declare function validateAndParsePublicKey({ publicKey: orgPubKey, transformSol, }: {
    publicKey: PublicKeyish;
    transformSol?: boolean;
}): PublicKey;
export declare const isSolWSolToken: (token?: UnifiedCurrency | null) => boolean;
export declare const isSol: (mint: string) => boolean;
export declare const isWSol: (mint: string) => boolean;
export declare const isSolWSol: (mint: string) => boolean;
export declare const solToWSol: (key: string) => string;
export declare const wSolToSol: (key: string) => string;
//# sourceMappingURL=utils.d.ts.map