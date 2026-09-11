//! Host UI bridge for Trezor One PIN matrix (and similar prompts).
//!
//! USB runs on a worker thread and blocks in [`Self::request_pin`] until the TUI
//! submits digits via [`Self::submit_pin`]. Digits typed before the device asks
//! are buffered. Never log PIN values.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Mutex;

use crate::error::WalletError;

type PinReply = Result<String, WalletError>;

/// Shared between the Trezor USB worker and the TUI event loop.
#[derive(Default)]
pub struct TrezorUiBridge {
    need_pin: AtomicBool,
    /// Rendezvous sender waiting for the user's matrix digits.
    pin_slot: Mutex<Option<SyncSender<PinReply>>>,
    /// PIN entered on the TUI before the device asked (Trezor One connect race).
    early_pin: Mutex<Option<PinReply>>,
}

impl TrezorUiBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// USB worker: take a buffered PIN or wait for the TUI matrix.
    pub fn request_pin(&self) -> Result<String, WalletError> {
        if let Some(early) = self
            .early_pin
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            return early;
        }

        let (tx, rx) = mpsc::sync_channel::<PinReply>(1);
        {
            let mut slot = self
                .pin_slot
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *slot = Some(tx);
        }
        self.need_pin.store(true, Ordering::SeqCst);
        let result = rx.recv().map_err(|_| {
            WalletError::HardwareUnsupported("Trezor PIN entry interrupted".into())
        })?;
        self.need_pin.store(false, Ordering::SeqCst);
        result
    }

    /// TUI: true while a worker is blocked in [`Self::request_pin`].
    pub fn pin_pending(&self) -> bool {
        self.need_pin.load(Ordering::SeqCst)
    }

    /// TUI: deliver matrix digits (`"1"`–`"9"` positions) or cancel.
    ///
    /// If the USB worker has not called [`Self::request_pin`] yet, the value is
    /// buffered for the next request (so the numpad can stay visible during connect).
    pub fn submit_pin(&self, pin: PinReply) {
        self.need_pin.store(false, Ordering::SeqCst);
        let sender = self
            .pin_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(tx) = sender {
            let _ = tx.send(pin);
            return;
        }
        *self
            .early_pin
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(pin);
    }

    /// Drop any buffered / in-flight PIN wait (Esc on connect screen).
    pub fn cancel_pin(&self) {
        self.submit_pin(Err(WalletError::HardwareUnsupported(
            "Trezor PIN entry cancelled".into(),
        )));
    }
}
