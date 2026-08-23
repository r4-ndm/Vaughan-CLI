//! Tests for the TUI's unlock view against a real wallet (anvil-backed).
//!
//! Drives the real `UnlockView` with real key events and renders it headlessly
//! via ratatui's `TestBackend`. Unlocking decrypts the vault offline; the
//! wallet itself is funded from the anvil dev mnemonic so the address is
//! meaningful and the `accountsChanged` event dApps receive can be asserted.
//!
//! After a correct password the user must pick a session mode (FR-5.1) before
//! reaching the dashboard.

mod common;

use common::{funded_wallet, render_frame, Anvil, PASSWORD};
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;
use tokio::runtime::Handle;
use vaughan_core::core::{OperatingMode, WalletState};
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

/// Correct password unlocks the vault and shows the session-mode picker
/// (does not navigate yet). `accountsChanged` still fires on unlock.
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

    assert!(
        matches!(outcome, KeyOutcome::Consumed),
        "password success stays on mode picker, got {outcome:?}"
    );
    assert!(wallet.is_unlocked(), "wallet must be unlocked");

    let notification = rx.try_recv().expect("accountsChanged event must fire");
    let value: Value = serde_json::from_str(&notification).unwrap();
    assert_eq!(value["method"], "accountsChanged");
    assert_eq!(
        value["params"][0].as_str().unwrap().to_lowercase(),
        wallet.active_address().unwrap().to_string().to_lowercase()
    );

    let text = render(&view, &wallet);
    assert!(
        text.contains("Session mode") || text.contains("operating mode"),
        "must show the session mode picker:\n{text}"
    );
    assert!(
        text.contains("Classic Human") || text.contains("1 —"),
        "must list Human mode:\n{text}"
    );
}

/// After unlock, pressing `1` selects Human mode and navigates to the dashboard.
#[test]
fn unlock_view_mode_select_human_goes_to_dashboard() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = locked_wallet(&anvil, dir.path());

    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = UnlockView::default();

    type_text(&mut view, PASSWORD, &mut wallet, &handle, &events);
    assert!(matches!(
        view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events),
        KeyOutcome::Consumed
    ));

    let outcome = view.handle_key(key(KeyCode::Char('1')), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Navigate(Screen::Dashboard)));
    assert_eq!(wallet.operating_mode(), OperatingMode::HumanOnly);
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
