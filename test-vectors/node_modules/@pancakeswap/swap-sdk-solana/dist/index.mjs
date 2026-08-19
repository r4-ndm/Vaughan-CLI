import { NonEVMChainId } from '@pancakeswap/chains';
import { TOKEN_PROGRAM_ID } from '@solana/spl-token';
import { PublicKey } from '@solana/web3.js';
import { SPLToken, SPLNativeCurrency } from '@pancakeswap/swap-sdk-core';

// src/constants.ts
var WSOLMint = new PublicKey("So11111111111111111111111111111111111111112");
var SOLMint = PublicKey.default;
var SOL_INFO = {
  chainId: NonEVMChainId.SOLANA,
  address: PublicKey.default.toBase58(),
  programId: TOKEN_PROGRAM_ID.toBase58(),
  decimals: 9,
  symbol: "SOL",
  name: "Solana",
  logoURI: `https://img-v1.raydium.io/icon/So11111111111111111111111111111111111111112.png`,
  tags: [],
  priority: 2,
  type: "raydium",
  extensions: {
    coingeckoId: "solana"
  }
};
var TOKEN_WSOL = {
  chainId: NonEVMChainId.SOLANA,
  address: "So11111111111111111111111111111111111111112",
  programId: TOKEN_PROGRAM_ID.toBase58(),
  decimals: 9,
  symbol: "WSOL",
  name: "Wrapped SOL",
  logoURI: `https://img-v1.raydium.io/icon/So11111111111111111111111111111111111111112.png`,
  tags: [],
  priority: 2,
  type: "raydium",
  extensions: {
    coingeckoId: "solana"
  }
};
var WSOL = new SPLToken(TOKEN_WSOL);
var SPLNative = class extends SPLNativeCurrency {
  constructor({
    chainId,
    decimals,
    name,
    symbol,
    programId,
    address,
    logoURI
  }) {
    super(chainId, decimals, symbol, name);
    this.programId = programId;
    this.address = address;
    this.logoURI = logoURI;
  }
  // eslint-disable-next-line class-methods-use-this
  get wrapped() {
    return WSOL;
  }
  equals(other) {
    return other.isNative && other.chainId === this.chainId;
  }
};
var SOL = new SPLNative(SOL_INFO);
function tryParsePublicKey(v) {
  try {
    return new PublicKey(v);
  } catch (e) {
    return v;
  }
}
function validateAndParsePublicKey({
  publicKey: orgPubKey,
  transformSol
}) {
  const publicKey = tryParsePublicKey(orgPubKey.toString());
  if (publicKey instanceof PublicKey) {
    if (transformSol && publicKey.equals(SOLMint))
      return WSOLMint;
    return publicKey;
  }
  if (transformSol && publicKey.toString() === SOLMint.toBase58())
    return WSOLMint;
  if (typeof publicKey === "string") {
    if (publicKey === PublicKey.default.toBase58())
      return PublicKey.default;
    try {
      const key = new PublicKey(publicKey);
      return key;
    } catch {
      throw new Error("invalid public key");
    }
  }
  throw new Error("invalid public key");
}
var isSolWSolToken = (token) => {
  if (!token)
    return false;
  if (token.chainId !== NonEVMChainId.SOLANA && token.chainId !== 101)
    return false;
  if (token.isNative)
    return true;
  return token.address.toString() === TOKEN_WSOL.address || token.address.toString() === SOL_INFO.address;
};
var isSol = (mint) => mint === SOLMint.toBase58();
var isWSol = (mint) => mint === WSOLMint.toBase58();
var isSolWSol = (mint) => isSol(mint) || isWSol(mint);
var solToWSol = (key) => key === SOLMint.toBase58() ? WSOLMint.toBase58() : key;
var wSolToSol = (key) => key === WSOLMint.toBase58() ? SOLMint.toBase58() : key;

export { SOL, SOLMint, SOL_INFO, SPLNative, TOKEN_WSOL, WSOL, WSOLMint, isSol, isSolWSol, isSolWSolToken, isWSol, solToWSol, tryParsePublicKey, validateAndParsePublicKey, wSolToSol };
//# sourceMappingURL=out.js.map
//# sourceMappingURL=index.mjs.map