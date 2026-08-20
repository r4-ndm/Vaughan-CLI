//! Tests for the TUI's dashboard view against a real anvil node.
//!
//! The dashboard shows the active address, network, and a live native
//! balance (`r` refreshes). Rendered headlessly via ratatui's `TestBackend`;
//! balance fetches hit anvil through the wallet, and the `l` lock shortcut's
//! dApp-facing `accountsChanged` event is asserted on a real `EventBus`.

mod common;

use common::{funded_wallet, render_frame, Anvil};
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use vaughan_tui::app::{KeyOutcome, Screen};
use vaughan_tui::views::DashboardView;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

fn render(view: &DashboardView, wallet: &WalletState) -> String {
    render_frame(100, 24, |f| view.render(f, f.area(), wallet))
}

fn runtime_handle() -> (tokio::runtime::Runtime, Handle) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    (rt, handle)
}

/// Address, network (with testnet marker), balance, and the shortcut bar are
/// all rendered.
#[test]
fn dashboard_view_renders_address_network_and_balance() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let wallet = funded_wallet(dir.path(), &anvil);
    let address = wallet.active_address().unwrap().to_string();
    let balance = handle.block_on(wallet.balance()).unwrap();

    let view = DashboardView::with_balance(Ok(balance.clone()));
    let text = render(&view, &wallet);

    assert!(
        text.to_lowercase().contains(&address.to_lowercase()),
        "must render the active address:\n{text}"
    );
    assert!(
        text.contains("PulseChain Testnet V4 (testnet)"),
        "must render the network with the testnet marker:\n{text}"
    );
    assert!(
        text.contains(&format!("{} tPLS", balance.formatted)),
        "must render the live balance:\n{text}"
    );
    for hint in ["s send", "k keys", "w dapps", "a assets", "refresh", "lock"] {
        assert!(
            text.contains(hint),
            "must render shortcut '{hint}':\n{text}"
        );
    }
}

/// A freshly built dashboard shows no balance; pressing `r` fetches it from
/// the node and renders it.
#[test]
fn dashboard_view_refresh_fetches_balance() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let events = EventBus::new();
    let mut view = DashboardView::default();

    // No balance yet — a dash.
    assert!(
        render(&view, &wallet).contains("Balance:  —"),
        "fresh dashboard must show no balance"
    );

    // `r` schedules a balance refresh job (app applies it on the UI thread).
    let outcome = view.handle_key(key(KeyCode::Char('r')), &mut wallet, &handle, &events);
    match outcome {
        KeyOutcome::StartJob(vaughan_tui::jobs::UiJob::RefreshBalance) => {
            view.apply_balance(handle.block_on(wallet.balance()));
        }
        other => panic!("expected RefreshBalance job, got {other:?}"),
    }
    let expected = handle.block_on(wallet.balance()).unwrap();
    let text = render(&view, &wallet);
    assert!(
        text.contains(&format!("{} tPLS", expected.formatted)),
        "refresh must show the live balance:\n{text}"
    );
    assert!(
        !text.contains("Balance:  —"),
        "balance placeholder must be gone"
    );
}

/// `l` locks the wallet, navigates to the unlock screen, and tells connected
/// dApps the account list is now empty.
#[test]
fn dashboard_view_lock_shortcut_locks_and_publishes_event() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let events = EventBus::new();
    let mut rx = events.subscribe();
    let mut view = DashboardView::default();

    assert!(wallet.is_unlocked());
    let outcome = view.handle_key(key(KeyCode::Char('l')), &mut wallet, &handle, &events);

    assert!(matches!(outcome, KeyOutcome::Navigate(Screen::Unlock)));
    assert!(!wallet.is_unlocked(), "wallet must be locked");

    let notification = rx.try_recv().expect("accountsChanged event must fire");
    let value: Value = serde_json::from_str(&notification).unwrap();
    assert_eq!(value["method"], "accountsChanged");
    assert_eq!(
        value["params"].as_array().unwrap().len(),
        0,
        "dApps must see an empty account list after locking"
    );
}

/// `s` / `b` / `v` / `n` / `k` / `a` / `c` navigate to send, batch, receive, settings, keys, assets, browser.
#[test]
fn dashboard_view_shortcuts_navigate() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let events = EventBus::new();
    let mut view = DashboardView::default();

    assert!(matches!(
        view.handle_key(key(KeyCode::Char('s')), &mut wallet, &handle, &events),
        KeyOutcome::Navigate(Screen::Send)
    ));
    assert!(matches!(
        view.handle_key(key(KeyCode::Char('b')), &mut wallet, &handle, &events),
        KeyOutcome::Navigate(Screen::AaSend)
    ));
    assert!(matches!(
        view.handle_key(key(KeyCode::Char('v')), &mut wallet, &handle, &events),
        KeyOutcome::Navigate(Screen::Receive)
    ));
    assert!(matches!(
        view.handle_key(key(KeyCode::Char('n')), &mut wallet, &handle, &events),
        KeyOutcome::Navigate(Screen::Settings)
    ));
    assert!(matches!(
        view.handle_key(key(KeyCode::Char('a')), &mut wallet, &handle, &events),
        KeyOutcome::Navigate(Screen::Assets)
    ));
    assert!(matches!(
        view.handle_key(key(KeyCode::Char('k')), &mut wallet, &handle, &events),
        KeyOutcome::Navigate(Screen::Keys)
    ));
    assert!(matches!(
        view.handle_key(key(KeyCode::Char('c')), &mut wallet, &handle, &events),
        KeyOutcome::Navigate(Screen::Browser)
    ));
}

/// A locked wallet renders a placeholder instead of the address.
#[test]
fn dashboard_view_locked_wallet_shows_placeholder() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    wallet.lock();

    let text = render(&DashboardView::default(), &wallet);
    assert!(
        text.contains("(locked)"),
        "locked wallet must not leak the address:\n{text}"
    );
}
