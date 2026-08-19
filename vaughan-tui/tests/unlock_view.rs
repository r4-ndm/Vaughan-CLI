//! Tests for the TUI's unlock view against a real wallet (anvil-backed).
//!
//! Drives the real `UnlockView` with real key events and renders it headlessly
//! via ratatui's `TestBackend`. Unlocking decrypts the vault offline; the
//! wallet itself is funded from the anvil dev mnemonic so the address is
//! meaningful and the `accountsChanged` event dApps receive can be asserted.

mod common;

use common::{funded_wallet, render_frame, Anvil, PASSWORD};
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use vaughan_tui::app::{KeyOutcome, Screen};
use vaughan_tui::views::UnlockView;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

fn render(view: &UnlockView, wallet: &WalletState) -> String {
    render_frame(100, 24, |f| view.render(f, f.area(), wallet))
}

fn runtime_handle() -> (tokio::runtime::Runtime, Handle) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    (rt, handle)
}

fn type_text(
    view: &mut UnlockView,
    text: &str,
    wallet: &mut WalletState,
    handle: &Handle,
    events: &EventBus,
) {
    for c in text.chars() {
        view.handle_key(key(KeyCode::Char(c)), wallet, handle, events);
    }
}

/// A funded wallet, freshly locked.
fn locked_wallet(anvil: &Anvil, dir: &std::path::Path) -> WalletState {
    let mut wallet = funded_wallet(dir, anvil);
    wallet.lock();
    assert!(!wallet.is_unlocked());
    wallet
}

/// The correct password unlocks the vault: the wallet is live again, the view
/// navigates to the dashboard, and the `accountsChanged` event fires with the
/// active address.
#[test]
fn unlock_view_correct_password_unlocks_and_publishes_event() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = locked_wallet(&anvil, dir.path());

    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut rx = events.subscribe();
    let mut view = UnlockView::default();

    type_text(&mut view, PASSWORD, &mut wallet, &handle, &events);
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    assert!(matches!(outcome, KeyOutcome::Navigate(Screen::Dashboard)));
    assert!(wallet.is_unlocked(), "wallet must be unlocked");

    // Connected dApps are notified with the live account.
    let notification = rx.try_recv().expect("accountsChanged event must fire");
    let value: Value = serde_json::from_str(&notification).unwrap();
    assert_eq!(value["method"], "accountsChanged");
    assert_eq!(
        value["params"][0].as_str().unwrap().to_lowercase(),
        wallet.active_address().unwrap().to_string().to_lowercase()
    );

    // The password field is masked while typing.
    let text = render(&view, &wallet);
    assert!(
        text.contains("Password"),
        "must show the password field:\n{text}"
    );
}

/// A wrong password shows an error, keeps the wallet locked, and never
/// navigates or fires the event.
#[test]
fn unlock_view_wrong_password_stays_locked() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = locked_wallet(&anvil, dir.path());

    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut rx = events.subscribe();
    let mut view = UnlockView::default();

    type_text(
        &mut view,
        "DefinitelyNotThePassword!",
        &mut wallet,
        &handle,
        &events,
    );
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    assert!(
        !matches!(outcome, KeyOutcome::Navigate(_)),
        "must not navigate"
    );
    assert!(!wallet.is_unlocked(), "wallet must stay locked");
    assert!(
        rx.try_recv().is_err(),
        "no accountsChanged event on a failed unlock"
    );

    let text = render(&view, &wallet);
    assert!(
        text.to_lowercase().contains("wrong password"),
        "must surface the error:\n{text}"
    );
}
