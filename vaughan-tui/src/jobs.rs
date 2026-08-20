//! Background UI jobs so RPC work never `block_on`s the TUI thread.

use vaughan_core::chains::{Balance, Fee};
use vaughan_core::core::StealthSendResult;
use vaughan_core::error::WalletError;
use vaughan_core::security::stealth::StealthAnnouncement;

/// Work the app should spawn on the tokio runtime.
pub enum UiJob {
    RefreshBalance,
    RefreshAssets,
    EstimateFee {
        to: String,
        value_wei: String,
    },
    EstimateTokenFee {
        token: String,
        to: String,
        amount: String,
    },
    SendWithFee {
        to: String,
        value_wei: String,
        fee: Fee,
    },
    Send {
        to: String,
        value_wei: String,
    },
    SendToken {
        token: String,
        to: String,
        amount: String,
    },
    SendTokenWithFee {
        token: String,
        to: String,
        amount: String,
        fee: Fee,
    },
    SendStealth {
        announcement: StealthAnnouncement,
        value_wei: String,
    },
}

/// Result delivered back to the UI thread via an unbounded channel.
pub enum UiJobResult {
    Balance(Result<Balance, WalletError>),
    Assets(Result<Vec<Balance>, WalletError>),
    Fee(Result<Fee, WalletError>),
    Send(Result<String, WalletError>),
    SendStealth(Result<StealthSendResult, WalletError>),
}

/// Frames for a braille spinner (no extra deps).
pub fn spinner_frame(tick: u64) -> char {
    const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[(tick as usize) % FRAMES.len()]
}
