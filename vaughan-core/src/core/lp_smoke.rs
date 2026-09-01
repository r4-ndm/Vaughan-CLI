//! Known wiz4rd V3 LP pools on Pulse testnet (943) for smoke tests and agent tours.
//!
//! Live checks (skipped in default CI):
//! ```sh
//! cargo test -p vaughan-core --test lp_smoke_943 -- --ignored --nocapture
//! ```

use super::DexVenue;

/// Default public RPC for Pulse testnet v4 smoke tests.
pub const RPC_943: &str = "https://rpc.v4.testnet.pulsechain.com";

/// One catalogued pair + the fee tier where the pool is live on 943.
#[derive(Debug, Clone, Copy)]
pub struct LpSmoke943Pair {
    pub label: &'static str,
    /// Sorted token0 (`token0 < token1` on-chain).
    pub token0: &'static str,
    pub token1: &'static str,
    /// Fee tier (bps) where [`super::v3_pool_lifecycle`] is `Ready`.
    pub fee: u32,
    /// Default fee the TUI sets via [`apply_initial_fee_defaults`] (943 wiz4rd = 500).
    pub tui_default_fee: u32,
}

/// On-chain wiz4rd V3 pools verified on 943 — keep aligned with agent LP tours.
pub const LP_SMOKE_943: &[LpSmoke943Pair] = &[
    LpSmoke943Pair {
        label: "JIM/JANE",
        token0: "0x28Bc040cE32d78aFACb214f5460Adc2bbdaC6B59", // JANE
        token1: "0xc6ca0621683db4a03e31ad77e1d63eb3a03acbba", // JIM
        fee: 100,
        tui_default_fee: 500,
    },
    LpSmoke943Pair {
        label: "BOB/JANE",
        token0: "0x15de8ae884726f37ec90824f825d723ac93c8b77", // BOB
        token1: "0x28Bc040cE32d78aFACb214f5460Adc2bbdaC6B59", // JANE
        fee: 20_000,
        tui_default_fee: 500,
    },
    LpSmoke943Pair {
        label: "WZRD/WPLS",
        token0: "0x29bab93456c0E97EE931C1554c7C215480aa7766", // WZRD
        token1: "0x70499adEBB11Efd915E3b69E700c331778628707", // WPLS
        fee: 500,
        tui_default_fee: 500,
    },
];

/// Venue used for all [`LP_SMOKE_943`] entries today.
pub const LP_SMOKE_943_VENUE: DexVenue = DexVenue::Wiz4rd;
