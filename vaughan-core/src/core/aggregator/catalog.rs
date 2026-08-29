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
            Self::Piteas => "Piteas",
            Self::SwitchWin => "Switch.win",
            Self::Empseal => "Empseal",
            Self::NineMm9x => "9mm 9X",
            Self::Curv => "Jolt/CURV",
            Self::InternetMoney => "Int.Money",
            Self::LibertyX => "LibertyX",
            Self::PortalX => "PortalX",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Self::SquirrelSwap => "Brain agg · public API, no key (api.squirrelswap.pro)",
            Self::PulseSwap => "PulseChain swap aggregator · public quotes",
            Self::Piteas => "Pathfinder DEX aggregator · public SDK beta",
            Self::SwitchWin => "aggregator + limit orders · needs x-api-key",
            Self::Empseal => "EmpX · on-chain Alloy path-find",
            Self::NineMm9x => "9mm multi-chain aggregator · no public developer quote API",
            Self::Curv => "Jolt / CURV · Switch routing (needs Switch API key)",
            Self::InternetMoney => "multi-chain wallet aggregator · not wired",
            Self::LibertyX => "use Bridge (f) — LibertySwap cross-chain",
            Self::PortalX => "cross-chain portal · not wired",
        }
    }

    pub fn access(self) -> AggAccess {
        match self {
            Self::SquirrelSwap | Self::PulseSwap | Self::Piteas | Self::Empseal => {
                AggAccess::LiveNoKey
            }
            Self::SwitchWin => AggAccess::NeedsApiKey("requires Switch x-api-key"),
            Self::NineMm9x => AggAccess::ListedOnly("no public developer quote API"),
            Self::Curv => AggAccess::NeedsApiKey("Switch routing — same x-api-key as Switch.win"),
            Self::LibertyX => AggAccess::ListedOnly("Bridge screen · LibertySwap"),
            Self::InternetMoney | Self::PortalX => {
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

    /// Human-facing swap UI for VB agent control (no developer API key).
    ///
    /// Browserless Ag/MCP may still use HTTP clients where `is_live()`; this is
    /// the side door for Switch, CURV, and any venue with a web app.
    /// Deep link or swap page preset for PLS → HEX on PulseChain (369).
    ///
    /// **Same-chain only.** LibertyX (bridge / USDC) and Internet Money (wallet
    /// shell) are excluded — use Bridge (`f`) or `web_url()` for those products.
    pub fn web_url_pls_hex(self) -> Option<String> {
        const HEX: &str = "0x2b591e99afE9f32eAA6214f7B7629768c40Eeb39";
        const NATIVE: &str = "0x0000000000000000000000000000000000000000";
        match self {
            Self::SquirrelSwap => Some("https://app.squirrelswap.pro/#/swap".into()),
            Self::PulseSwap => Some(format!(
                "https://pulseswap.io/?chain=pulsechain&from={NATIVE}&to={HEX}&amount=1"
            )),
            Self::Piteas => Some("https://app.piteas.io/".into()),
            Self::SwitchWin | Self::Curv => Some("https://www.switch.win/dapp".into()),
            Self::NineMm9x => Some("https://9x.9mm.pro/#/swap?chainId=369".into()),
            Self::LibertyX | Self::InternetMoney | Self::Empseal | Self::PortalX => None,
        }
    }

    pub fn web_url(self) -> Option<&'static str> {
        match self {
            Self::SquirrelSwap => Some("https://app.squirrelswap.pro/#/swap"),
            Self::PulseSwap => Some("https://pulseswap.io/?chain=pulsechain"),
            Self::Piteas => Some("https://app.piteas.io/"),
            Self::SwitchWin => Some("https://www.switch.win/dapp"),
            Self::Empseal => None, // on-chain path-find only — no public swap UI
            Self::NineMm9x => Some("https://9x.9mm.pro/#/swap?chainId=369"),
            Self::Curv => Some("https://www.switch.win/dapp"), // Jolt/CURV — Switch routing UI
            Self::InternetMoney => Some("https://internetmoney.io/"),
            Self::LibertyX => Some("https://libertyswap.finance/"), // bridge — not same-chain Ag
            Self::PortalX => None,
        }
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
        assert!(!AggVenue::Curv.is_live());
        assert!(matches!(AggVenue::Curv.access(), AggAccess::NeedsApiKey(_)));
        assert!(AggVenue::Empseal.is_live());
        assert!(matches!(AggVenue::Empseal.access(), AggAccess::LiveNoKey));
    }

    #[test]
    fn cycle_wraps() {
        assert_eq!(AggVenue::SquirrelSwap.next(), AggVenue::PulseSwap);
        assert_eq!(AggVenue::PortalX.next(), AggVenue::SquirrelSwap);
    }

    #[test]
    fn web_urls_for_vb_human_path() {
        assert!(AggVenue::SquirrelSwap
            .web_url()
            .unwrap()
            .contains("squirrelswap"));
        assert!(AggVenue::SwitchWin
            .web_url()
            .unwrap()
            .contains("switch.win"));
        assert!(AggVenue::Empseal.web_url().is_none());
        assert!(AggVenue::NineMm9x.web_url().unwrap().contains("9x.9mm.pro"));
        assert!(AggVenue::LibertyX.web_url_pls_hex().is_none());
        assert!(AggVenue::InternetMoney.web_url_pls_hex().is_none());
    }
}
