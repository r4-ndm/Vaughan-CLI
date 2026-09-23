//! DCA plan types — no I/O, no network.

use alloy::primitives::Address;
use serde::{Deserialize, Serialize};
use vaughan_core::core::{
    parse_dex_venue_label, venue_slug, venue_swap_router, AggAccess, AggVenue, DexProtocol,
    DexVenue, AGG_VENUES, DEX_VENUES,
};

/// Filename next to `sentient-policy.toml` under the profile dir.
pub const DCA_PLANS_JSON: &str = "dca-plans.json";

/// Minimum interval between slices (seconds).
pub const MIN_INTERVAL_SECS: u64 = 15 * 60;

/// Consecutive slice failures before the plan auto-pauses.
pub const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// Max retained slice log rows per plan.
pub const MAX_SLICE_LOG: usize = 32;

/// Plan lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DcaStatus {
    #[default]
    Active,
    Paused,
    Done,
    Cancelled,
}

impl DcaStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

/// What wakes a plan. Phase 1 is time-only; Phase 2 adds indicator variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DcaTrigger {
    /// Fire when `now >= next_due_at`, then advance by `interval_secs`.
    Time { interval_secs: u64 },
}

impl DcaTrigger {
    pub fn interval_secs(&self) -> Option<u64> {
        match self {
            Self::Time { interval_secs } => Some(*interval_secs),
        }
    }
}

/// Where each slice routes: aggregator **or** direct DEX.
///
/// Tagged `kind` so meme coins missing from aggs can use PulseX / 9inch / etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DcaVenue {
    /// Aggregator (`name`: `auto` / `squirrel` / `pulseswap` / `piteas` / `empx` / `9mm`).
    Agg { name: String },
    /// Direct catalogued DEX (`name`: `pulsex`, `9inch`, …; `protocol`: `v2` or `v3`).
    Dex {
        name: String,
        #[serde(default = "default_dex_protocol")]
        protocol: String,
    },
}

fn default_dex_protocol() -> String {
    "v2".into()
}

impl Default for DcaVenue {
    fn default() -> Self {
        Self::Agg {
            name: "auto".into(),
        }
    }
}

impl DcaVenue {
    /// Short label for list rows / confirm.
    pub fn display_label(&self) -> String {
        match self {
            Self::Agg { name } => {
                if name.eq_ignore_ascii_case("auto") {
                    "Agg · auto (Squirrel)".into()
                } else {
                    format!("Agg · {name}")
                }
            }
            Self::Dex { name, protocol } => format!("DEX · {name} · {protocol}"),
        }
    }

    /// Validate labels resolve to a live router on `chain_id` (DEX) or live agg.
    pub fn validate_for_chain(&self, chain_id: u64) -> Result<(), String> {
        match self {
            Self::Agg { name } => {
                if !vaughan_core::core::agg_routers_supported_on(chain_id) {
                    return Err(format!(
                        "aggregator DCA is PulseChain mainnet (369) only — chain {chain_id} has no agg routers"
                    ));
                }
                let _ = resolve_agg_name(name)?;
                Ok(())
            }
            Self::Dex { name, protocol } => {
                let venue = parse_dex_venue_label(name)
                    .ok_or_else(|| format!("unknown DEX venue '{name}'"))?;
                let proto = parse_dex_protocol(protocol)?;
                if proto != DexProtocol::V2 {
                    return Err(
                        "DCA direct DEX currently supports V2 only — pick route=agg for V3".into(),
                    );
                }
                if venue_swap_router(venue, proto, chain_id).is_none() {
                    return Err(format!(
                        "DEX {} {} has no swap router on chain {chain_id}",
                        venue_slug(venue),
                        proto.label()
                    ));
                }
                Ok(())
            }
        }
    }
}

/// Resolved route ready for quoting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DcaRouteResolved {
    Agg(AggVenue),
    Dex {
        venue: DexVenue,
        protocol: DexProtocol,
    },
}

impl DcaVenue {
    pub fn resolve(&self) -> Result<DcaRouteResolved, String> {
        match self {
            Self::Agg { name } => Ok(DcaRouteResolved::Agg(resolve_agg_name(name)?)),
            Self::Dex { name, protocol } => {
                let venue = parse_dex_venue_label(name)
                    .ok_or_else(|| format!("unknown DEX venue '{name}'"))?;
                let protocol = parse_dex_protocol(protocol)?;
                Ok(DcaRouteResolved::Dex { venue, protocol })
            }
        }
    }
}

fn resolve_agg_name(raw: &str) -> Result<AggVenue, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "auto" | "squirrel" | "squirrelswap" => Ok(AggVenue::SquirrelSwap),
        "pulseswap" | "pulse" => Ok(AggVenue::PulseSwap),
        "piteas" => Ok(AggVenue::Piteas),
        "empx" | "empseal" => Ok(AggVenue::Empseal),
        "9mm" | "ninemm" | "ninemm9x" => Ok(AggVenue::NineMm9x),
        other => Err(format!(
            "unknown aggregator '{other}' — use auto, squirrel, pulseswap, piteas, empx, or 9mm"
        )),
    }
}

fn parse_dex_protocol(raw: &str) -> Result<DexProtocol, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "v2" | "2" => Ok(DexProtocol::V2),
        "v3" | "3" => Ok(DexProtocol::V3),
        other => Err(format!("unknown DEX protocol '{other}' — use v2 or v3")),
    }
}

/// Live no-key aggregators for ←/→ pickers (`auto` first).
pub fn agg_picker_labels() -> Vec<&'static str> {
    let mut out = vec!["auto"];
    for v in AGG_VENUES {
        if !matches!(v.access(), AggAccess::LiveNoKey) {
            continue;
        }
        let slug = match v {
            AggVenue::SquirrelSwap => "squirrel",
            AggVenue::PulseSwap => "pulseswap",
            AggVenue::Piteas => "piteas",
            AggVenue::Empseal => "empx",
            AggVenue::NineMm9x => "9mm",
            _ => continue,
        };
        out.push(slug);
    }
    out
}

/// DEX venues with a swap router on `chain_id` for `protocol`.
pub fn dex_picker_slugs(chain_id: u64, protocol: DexProtocol) -> Vec<&'static str> {
    DEX_VENUES
        .iter()
        .copied()
        .filter(|v| venue_swap_router(*v, protocol, chain_id).is_some())
        .map(venue_slug)
        .collect()
}

/// One executed (or attempted) slice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceLog {
    /// Unix seconds.
    pub at: u64,
    pub ok: bool,
    /// Broadcast hash when broadcast, or empty / zero for dry-run.
    #[serde(default)]
    pub tx_hash: String,
    pub dry_run: bool,
    #[serde(default)]
    pub error: String,
    /// Amount spent this slice (wei decimal string).
    #[serde(default)]
    pub amount_in_wei: String,
}

/// Persisted recurring-buy plan (PLS → ERC-20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DcaPlan {
    pub id: String,
    /// Chain the plan was created for; fire refuses on mismatch.
    /// `0` = legacy unbound plan (never fires; re-create it).
    #[serde(default)]
    pub chain_id: u64,
    /// Active wallet address at create time (0x…); fire refuses if F3 changed.
    /// Empty = legacy unbound plan (never fires; re-create it).
    #[serde(default)]
    pub account: String,
    /// ERC-20 to buy (never native).
    pub token_out: String,
    /// Native PLS per slice, wei decimal string.
    pub slice_amount_wei: String,
    pub trigger: DcaTrigger,
    pub venue: DcaVenue,
    pub max_slippage_bps: u32,
    pub max_slices: u32,
    #[serde(default)]
    pub slices_done: u32,
    /// Cumulative native spent (wei decimal string).
    #[serde(default = "zero_wei")]
    pub spent_wei: String,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub status: DcaStatus,
    /// Unix seconds when the plan was created.
    pub created_at: u64,
    /// Next eligible fire time (unix seconds).
    pub next_due_at: u64,
    /// Per-plan paper trading (also respects `VAUGHAN_SENTIENT_DRY_RUN` on the trader).
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub slice_log: Vec<SliceLog>,
}

fn zero_wei() -> String {
    "0".into()
}

impl DcaPlan {
    /// Parsed token_out address.
    pub fn token_out_address(&self) -> Result<Address, String> {
        self.token_out
            .parse()
            .map_err(|e| format!("bad token_out: {e}"))
    }

    /// Parsed bound account address.
    pub fn account_address(&self) -> Result<Address, String> {
        self.account
            .parse()
            .map_err(|e| format!("bad account: {e}"))
    }

    pub fn slice_amount_u256(&self) -> Result<alloy::primitives::U256, String> {
        use std::str::FromStr;
        alloy::primitives::U256::from_str(self.slice_amount_wei.trim())
            .map_err(|e| format!("bad slice_amount_wei: {e}"))
    }

    pub fn spent_u256(&self) -> Result<alloy::primitives::U256, String> {
        use std::str::FromStr;
        alloy::primitives::U256::from_str(self.spent_wei.trim())
            .map_err(|e| format!("bad spent_wei: {e}"))
    }

    /// Hard budget: `slice * max_slices`.
    pub fn budget_wei(&self) -> Result<alloy::primitives::U256, String> {
        let slice = self.slice_amount_u256()?;
        Ok(slice.saturating_mul(alloy::primitives::U256::from(self.max_slices)))
    }
}

/// On-disk envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DcaPlanFile {
    #[serde(default)]
    pub plans: Vec<DcaPlan>,
}

/// Draft returned by `propose_dca_plan` for human approval before store write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DcaPlanProposal {
    pub proposal_id: String,
    pub llm_explanation: String,
    pub plan: DcaPlan,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agg_auto_resolves_squirrel() {
        let v = DcaVenue::Agg {
            name: "auto".into(),
        };
        assert_eq!(
            v.resolve().unwrap(),
            DcaRouteResolved::Agg(AggVenue::SquirrelSwap)
        );
    }

    #[test]
    fn dex_pulsex_v2_on_mainnet() {
        let v = DcaVenue::Dex {
            name: "pulsex".into(),
            protocol: "v2".into(),
        };
        v.validate_for_chain(369).unwrap();
        match v.resolve().unwrap() {
            DcaRouteResolved::Dex { venue, protocol } => {
                assert_eq!(venue, DexVenue::PulseX);
                assert_eq!(protocol, DexProtocol::V2);
            }
            _ => panic!("expected dex"),
        }
    }

    #[test]
    fn venue_serde_tagged() {
        let v = DcaVenue::Dex {
            name: "9inch".into(),
            protocol: "v2".into(),
        };
        let j = serde_json::to_string(&v).unwrap();
        assert!(j.contains("\"kind\":\"dex\""));
        let back: DcaVenue = serde_json::from_str(&j).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn trigger_serde_roundtrip() {
        let t = DcaTrigger::Time {
            interval_secs: 14400,
        };
        let j = serde_json::to_string(&t).unwrap();
        assert!(j.contains("time"));
        let back: DcaTrigger = serde_json::from_str(&j).unwrap();
        assert_eq!(back, t);
    }
}
