'use strict';

var chains = require('@pancakeswap/chains');
var splToken = require('@solana/spl-token');
var web3_js = require('@solana/web3.js');
var swapSdkCore = require('@pancakeswap/swap-sdk-core');

// src/constants.ts
var WSOLMint = new web3_js.PublicKey("So11111111111111111111111111111111111111112");
var SOLMint = web3_js.PublicKey.default;
var SOL_INFO = {
  chainId: chains.NonEVMChainId.SOLANA,
  address: web3_js.PublicKey.default.toBase58(),
  programId: splToken.TOKEN_PROGRAM_ID.toBase58(),
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
  chainId: chains.NonEVMChainId.SOLANA,
  address: "So11111111111111111111111111111111111111112",
  programId: splToken.TOKEN_PROGRAM_ID.toBase58(),
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
var WSOL = new swapSdkCore.SPLToken(TOKEN_WSOL);
var SPLNative = class extends swapSdkCore.SPLNativeCurrency {
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
    return new web3_js.PublicKey(v);
  } catch (e) {
    return v;
  }
}
function validateAndParsePublicKey({
  publicKey: orgPubKey,
  transformSol
}) {
  const publicKey = tryParsePublicKey(orgPubKey.toString());
  if (publicKey instanceof web3_js.PublicKey) {
    if (transformSol && publicKey.equals(SOLMint))
      return WSOLMint;
    return publicKey;
  }
  if (transformSol && publicKey.toString() === SOLMint.toBase58())
    return WSOLMint;
  if (typeof publicKey === "string") {
    if (publicKey === web3_js.PublicKey.default.toBase58())
      return web3_js.PublicKey.default;
    try {
      const key = new web3_js.PublicKey(publicKey);
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
  if (token.chainId !== chains.NonEVMChainId.SOLANA && token.chainId !== 101)
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

exports.SOL = SOL;
exports.SOLMint = SOLMint;
exports.SOL_INFO = SOL_INFO;
exports.SPLNative = SPLNative;
exports.TOKEN_WSOL = TOKEN_WSOL;
exports.WSOL = WSOL;
exports.WSOLMint = WSOLMint;
exports.isSol = isSol;
exports.isSolWSol = isSolWSol;
exports.isSolWSolToken = isSolWSolToken;
exports.isWSol = isWSol;
exports.solToWSol = solToWSol;
exports.tryParsePublicKey = tryParsePublicKey;
exports.validateAndParsePublicKey = validateAndParsePublicKey;
exports.wSolToSol = wSolToSol;
//# sourceMappingURL=out.js.map
//# sourceMappingURL=index.js.map