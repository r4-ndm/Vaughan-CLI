//! Profile and operating mode management.
//!
//! Provides the 3-tier operating mode hierarchy:
//! - [`OperatingMode::HumanOnly`]: Pure sovereign manual wallet (zero AI code initialized, zero LLM network calls).
//! - [`OperatingMode::AiAssisted`]: Advisor-only mode (propose-only, zero signing capability, unified human confirmation).
//! - [`OperatingMode::DegenTrader`]: Autonomous execution strictly inside an isolated burner sub-profile with hard circuit breakers.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The operating mode for a wallet session.
///
/// Immutably selected at startup or onboarding and locked for the lifetime of
/// the process session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OperatingMode {
    /// Pure Sovereign Manual Wallet: Zero AI modules initialized in memory, zero LLM network traffic.
    #[default]
    HumanOnly,
    /// AI Assistant Mode: AI is an Advisor only (Propose-only, zero signing capability, unified human confirmation).
    AiAssisted,
    /// Degen Trader Mode: Autonomous trading execution strictly inside an isolated burner sub-profile with hard circuit breakers.
    DegenTrader,
}

impl OperatingMode {
    /// Returns true if AI modules should be initialized.
    pub fn is_ai_enabled(&self) -> bool {
        matches!(self, Self::AiAssisted | Self::DegenTrader)
    }

    /// Returns true if autonomous execution without manual human prompts is permitted (Degen mode only).
    pub fn is_autonomous_execution_allowed(&self) -> bool {
        matches!(self, Self::DegenTrader)
    }

    /// Human-readable label for status bars and confirmation modals.
    pub fn display_label(&self) -> &'static str {
        match self {
            Self::HumanOnly => "Human Only (Cold/Manual)",
            Self::AiAssisted => "AI Assisted (Advisor)",
            Self::DegenTrader => "Degen Bot (Autonomous Trader)",
        }
    }
}

impl fmt::Display for OperatingMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_human_only() {
        assert_eq!(OperatingMode::default(), OperatingMode::HumanOnly);
        assert!(!OperatingMode::HumanOnly.is_ai_enabled());
        assert!(!OperatingMode::HumanOnly.is_autonomous_execution_allowed());
    }

    #[test]
    fn ai_assisted_is_ai_enabled_but_not_autonomous() {
        assert!(OperatingMode::AiAssisted.is_ai_enabled());
        assert!(!OperatingMode::AiAssisted.is_autonomous_execution_allowed());
    }

    #[test]
    fn degen_trader_is_both_ai_and_autonomous() {
        assert!(OperatingMode::DegenTrader.is_ai_enabled());
        assert!(OperatingMode::DegenTrader.is_autonomous_execution_allowed());
    }
}
