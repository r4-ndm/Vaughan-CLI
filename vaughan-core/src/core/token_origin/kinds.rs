//! Origin classification for PulseChain catalogued tokens.

use serde::Serialize;

/// How a known token exists on PulseChain (e*/p* community naming).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenOriginKind {
    /// Bridged from Ethereum via bridge.pulsechain.com (e*).
    BridgedFromEth,
    /// Native or PulseChain-origin (WPLS, PLSX, …).
    PulseNative,
    /// State-fork copy at the Ethereum address (typically useless p*).
    StateFork,
    /// State-fork that is preferred on PulseChain (pHEX).
    PreferredStateFork,
}

impl TokenOriginKind {
    /// Short machine label for tool JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BridgedFromEth => "bridged_from_eth",
            Self::PulseNative => "pulse_native",
            Self::StateFork => "state_fork",
            Self::PreferredStateFork => "preferred_state_fork",
        }
    }
}
