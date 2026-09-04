//! HEX stake reads + calldata (PulseChain pHEX state-fork).
//!
//! Patterns inspired by [pulsechain-mcp](https://github.com/DavidFeder/pulsechain-mcp)
//! `hexStake` helpers; reimplemented in Rust (no TypeScript vendoring).
//!
//! - **pHEX** at the Ethereum HEX address is stakeable on PulseChain 369.
//! - **eHEX** is bridged ERC-20 only — no `stakeLists` / `globals`.
//!
//! Hearts use **8 decimals**. Not a price oracle.

mod calldata;
mod contract;
mod read;
mod types;

pub use calldata::{
    encode_stake_end, encode_stake_start, MAX_STAKE_DAYS, MIN_STAKE_DAYS, PHEX_HEARTS_DECIMALS,
};
pub use contract::{
    ehex_address, phex_address, resolve_hex_contract, HexContractKind, HexContractRef,
};
pub use read::{fetch_hex_global_state, fetch_hex_stakes_for_address};
pub use types::{
    HexGlobalState, HexSoftFail, HexStakeResult, HexStakeRow, HexStakesForAddress, HEX_STAKE_SOURCE,
};
