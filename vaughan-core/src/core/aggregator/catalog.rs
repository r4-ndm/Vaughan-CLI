//! Aggregator venue catalog (↑/↓ in the Ag TUI).

/// How Vaughan can talk to this aggregator today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggAccess {
    /// Public HTTP quote + calldata, no partner key.
    LiveNoKey,
    /// Documented API but needs a key / partner signup.
    NeedsApiKey(&'static str),
    /// Known product; no Vaughan quote path yet.
    ListedOnly(&'static str),
}

/// PulseChain (and multi-chain) aggregators shown in the Ag screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggVenue {
    /// Primary no-key focus: SquirrelSwap Brain (`api.squirrelswap.pro`).
    SquirrelSwap,
    PulseSwap,
    /// PulseSwap meta-aggregator branding (same public quote API).
    AggreGate,
    Piteas,
    SwitchWin,
    Empseal,
    NineMm9x,
    Curv,
    InternetMoney,
    LibertyX,
    PortalX,
}

/// ↑/↓ order — Squirrel first, then other live, then listed.
pub const AGG_VENUES: &[AggVenue] = &[
    AggVenue::SquirrelSwap,
    AggVenue::PulseSwap,
    AggVenue::Piteas,
    AggVenue::AggreGate,
    AggVenue::SwitchWin,
    AggVenue::Empseal,
    AggVenue::NineMm9x,
    AggVenue::Curv,
    AggVenue::InternetMoney,
    AggVenue::LibertyX,
    AggVenue::PortalX,
];

impl AggVenue {
    pub fn label(self) -> &'static str {
        match self {
            Self::SquirrelSwap => "Squirrel",
            Self::PulseSwap => "PulseSwap",
            Self::AggreGate => "AggreGate",
            Self::Piteas => "Piteas",
            Self::SwitchWin => "Switch.win",
            Self::Empseal => "Empseal",
            Self::NineMm9x => "9mm 9X",
            Self::Curv => "CURV",
            Self::InternetMoney => "Int.Money",
            Self::LibertyX => "LibertyX",
            Self::PortalX => "PortalX",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Self::SquirrelSwap => "Brain agg · public API, no key (api.squirrelswap.pro)",
            Self::PulseSwap => "PulseChain swap aggregator · public quotes",
            Self::AggreGate => "aggregator-of-aggregators · PulseSwap API",
            Self::Piteas => "Pathfinder DEX aggregator · public SDK beta",
            Self::SwitchWin => "aggregator + limit orders · needs x-api-key",
            Self::Empseal => "EmpX on-chain aggregator · no HTTP quote API",
            Self::NineMm9x => "9mm multi-chain aggregator · no public quote API yet",
            Self::Curv => "OTC + aggregator · not wired",
            Self::InternetMoney => "multi-chain wallet aggregator · not wired",
            Self::LibertyX => "cross-chain aggregator · not wired",
            Self::PortalX => "cross-chain portal · not wired",
        }
    }

    pub fn access(self) -> AggAccess {
        match self {
            Self::SquirrelSwap | Self::PulseSwap | Self::AggreGate | Self::Piteas => {
                AggAccess::LiveNoKey
            }
            Self::SwitchWin => AggAccess::NeedsApiKey("requires Switch x-api-key"),
            Self::Empseal => AggAccess::ListedOnly("on-chain EmpX SDK only"),
            Self::NineMm9x => AggAccess::ListedOnly("no published quote API"),
            Self::Curv => AggAccess::ListedOnly("OTC desk"),
            Self::InternetMoney | Self::LibertyX | Self::PortalX => {
                AggAccess::ListedOnly("cross-chain / wallet product")
            }
        }
    }

    pub fn next(self) -> Self {
        let i = AGG_VENUES.iter().position(|v| *v == self).unwrap_or(0);
        AGG_VENUES[(i + 1) % AGG_VENUES.len()]
    }

    pub fn prev(self) -> Self {
        let i = AGG_VENUES.iter().position(|v| *v == self).unwrap_or(0);
        AGG_VENUES[(i + AGG_VENUES.len() - 1) % AGG_VENUES.len()]
    }

    pub fn is_live(self) -> bool {
        matches!(self.access(), AggAccess::LiveNoKey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squirrel_is_default_live() {
        assert!(AggVenue::SquirrelSwap.is_live());
        assert_eq!(AGG_VENUES[0], AggVenue::SquirrelSwap);
        assert!(!AggVenue::SwitchWin.is_live());
    }

    #[test]
    fn cycle_wraps() {
        assert_eq!(AggVenue::SquirrelSwap.next(), AggVenue::PulseSwap);
        assert_eq!(AggVenue::PortalX.next(), AggVenue::SquirrelSwap);
    }
}
