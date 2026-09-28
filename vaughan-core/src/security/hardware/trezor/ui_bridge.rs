//! Host UI bridge for Trezor One PIN matrix and host passphrase entry.
//!
//! USB runs on a worker thread and blocks in [`Self::request_pin`] /
//! [`Self::request_passphrase`] until the TUI submits. Never log PIN or
//! passphrase values.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Mutex;

use secrecy::SecretString;
use zeroize::Zeroizing;

use crate::error::WalletError;

type PinReply = Result<String, WalletError>;
type PassphraseReply = Result<SecretString, WalletError>;

/// Shared between the Trezor USB worker and the TUI event loop.
#[derive(Default)]
pub struct TrezorUiBridge {
    need_pin: AtomicBool,
    need_passphrase: AtomicBool,
    /// True while USB is in / about to enter a ButtonRequest confirm.
    awaiting_button: AtomicBool,
    /// True while a sign waits for the user to plug the Trezor in.
    awaiting_device: AtomicBool,
    /// Host Esc requested abort (PIN / passphrase / confirm-on-device).
    abort: AtomicBool,
    /// Rendezvous sender waiting for the user's matrix digits.
    pin_slot: Mutex<Option<SyncSender<PinReply>>>,
    /// PIN entered on the TUI before the device asked (Trezor One connect race).
    early_pin: Mutex<Option<PinReply>>,
    passphrase_slot: Mutex<Option<SyncSender<PassphraseReply>>>,
    early_passphrase: Mutex<Option<PassphraseReply>>,
    /// Device session (holds the entered passphrase on-device) for one wallet
    /// address, so back-to-back signs do not re-ask. Keyed by address so a
    /// standard and a hidden wallet never share a session. Never logged.
    session: Mutex<Option<(String, Zeroizing<Vec<u8>>)>>,
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
            let mut slot = self.pin_slot.lock().unwrap_or_else(|e| e.into_inner());
            *slot = Some(tx);
        }
        self.need_pin.store(true, Ordering::SeqCst);
        let result = rx
            .recv()
            .map_err(|_| WalletError::HardwareUnsupported("Trezor PIN entry interrupted".into()))?;
        self.need_pin.store(false, Ordering::SeqCst);
        result
    }

    /// USB worker: Trezor One host passphrase (hidden wallet).
    ///
    /// Empty string is valid BIP-39 (standard wallet). Never log the value.
    pub fn request_passphrase(&self) -> Result<SecretString, WalletError> {
        if let Some(early) = self
            .early_passphrase
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            return early;
        }

        let (tx, rx) = mpsc::sync_channel::<PassphraseReply>(1);
        {
            let mut slot = self
                .passphrase_slot
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *slot = Some(tx);
        }
        self.need_passphrase.store(true, Ordering::SeqCst);
        let result = rx.recv().map_err(|_| {
            WalletError::HardwareUnsupported("Trezor passphrase entry interrupted".into())
        })?;
        self.need_passphrase.store(false, Ordering::SeqCst);
        result
    }

    /// TUI: true while a worker is blocked in [`Self::request_pin`].
    pub fn pin_pending(&self) -> bool {
        self.need_pin.load(Ordering::SeqCst)
    }

    /// TUI: true while a worker is blocked in [`Self::request_passphrase`].
    pub fn passphrase_pending(&self) -> bool {
        self.need_passphrase.load(Ordering::SeqCst)
    }

    /// USB: mark that a ButtonRequest confirm is in flight (for Esc handling).
    pub fn set_awaiting_button(&self, pending: bool) {
        self.awaiting_button.store(pending, Ordering::SeqCst);
    }

    /// TUI: true while USB is waiting on device button confirm.
    pub fn button_pending(&self) -> bool {
        self.awaiting_button.load(Ordering::SeqCst)
    }

    /// USB: mark that no Trezor is plugged in yet and the worker is polling.
    pub fn set_awaiting_device(&self, pending: bool) {
        self.awaiting_device.store(pending, Ordering::SeqCst);
    }

    /// TUI: true while the worker waits for the Trezor to be plugged in.
    pub fn device_pending(&self) -> bool {
        self.awaiting_device.load(Ordering::SeqCst)
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
        *self.early_pin.lock().unwrap_or_else(|e| e.into_inner()) = Some(pin);
    }

    /// TUI: deliver host passphrase or cancel (never log).
    pub fn submit_passphrase(&self, passphrase: PassphraseReply) {
        self.need_passphrase.store(false, Ordering::SeqCst);
        // Drop any early PIN buffered if the pad briefly reappeared after PIN.
        let _ = self
            .early_pin
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let sender = self
            .passphrase_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(tx) = sender {
            let _ = tx.send(passphrase);
            return;
        }
        *self
            .early_passphrase
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(passphrase);
    }

    /// Drop any buffered / in-flight PIN wait (Esc on connect / PIN pad).
    pub fn cancel_pin(&self) {
        self.submit_pin(Err(WalletError::HardwareUnsupported(
            "Trezor PIN entry cancelled".into(),
        )));
    }

    /// Drop in-flight passphrase wait.
    pub fn cancel_passphrase(&self) {
        self.submit_passphrase(Err(WalletError::HardwareUnsupported(
            "Trezor passphrase entry cancelled".into(),
        )));
    }

    /// Esc on PIN / passphrase / confirm-on-device: abort the in-flight USB interaction.
    pub fn request_abort(&self) {
        self.abort.store(true, Ordering::SeqCst);
        self.cancel_pin();
        self.cancel_passphrase();
    }

    /// USB worker: consume host abort (Esc). Returns true once per abort.
    pub fn take_abort(&self) -> bool {
        self.abort.swap(false, Ordering::SeqCst)
    }

    /// USB: session id to resume for `address`, if the last sign used it.
    pub(crate) fn cached_session(&self, address: &str) -> Option<Vec<u8>> {
        let slot = self.session.lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_ref() {
            Some((addr, id)) if addr.eq_ignore_ascii_case(address) => Some(id.to_vec()),
            _ => None,
        }
    }

    /// USB: remember the device session after a successful open for `address`.
    pub(crate) fn store_session(&self, address: &str, id: Vec<u8>) {
        *self.session.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((address.to_ascii_lowercase(), Zeroizing::new(id)));
    }

    /// Forget the device session (wallet lock, address mismatch).
    pub fn clear_session(&self) {
        *self.session.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_wait_flag_and_abort() {
        let bridge = TrezorUiBridge::new();
        assert!(!bridge.device_pending());
        bridge.set_awaiting_device(true);
        assert!(bridge.device_pending());
        bridge.request_abort();
        assert!(bridge.take_abort());
        assert!(!bridge.take_abort());
        bridge.set_awaiting_device(false);
        assert!(!bridge.device_pending());
    }

    #[test]
    fn session_is_scoped_to_one_address() {
        let bridge = TrezorUiBridge::new();
        let a = "0xAAaa000000000000000000000000000000000001";
        let b = "0xbbbb000000000000000000000000000000000002";
        assert!(bridge.cached_session(a).is_none());
        bridge.store_session(a, vec![1, 2, 3]);
        assert_eq!(
            bridge.cached_session(&a.to_lowercase()),
            Some(vec![1, 2, 3])
        );
        assert!(bridge.cached_session(b).is_none());
        bridge.clear_session();
        assert!(bridge.cached_session(a).is_none());
    }
}
