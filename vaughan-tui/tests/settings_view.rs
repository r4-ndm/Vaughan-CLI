//! Tests for the TUI's settings (network switching) view against a real
//! wallet. Rendered headlessly via ratatui's `TestBackend` (see
//! `tests/common/mod.rs`); network switches are verified both in the wallet
//! state and through the `chainChanged` event dApps would receive.

mod common;

use common::{funded_wallet, render_frame, Anvil};
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use vaughan_tui::app::{KeyOutcome, Screen};
use vaughan_tui::views::SettingsView;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

fn render(view: &SettingsView, wallet: &WalletState) -> String {
    render_frame(100, 30, |f| view.render(f, f.area(), wallet))
}

fn runtime_handle() -> (tokio::runtime::Runtime, Handle) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    (rt, handle)
}

/// Every built-in network is listed with its chain id, and the active one is
/// marked with `*`.
#[test]
fn settings_view_lists_networks_with_active_marker() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil); // active: pulsechain-testnet-v4 (943)
    let view = SettingsView::new(1);

    let text = render(&view, &wallet);
    for needle in [
        "PulseChain Mainnet",
        "chain 369",
        "PulseChain Testnet V4",
        "chain 943",
        "Ethereum Mainnet",
        "chain 1",
        "Base Mainnet",
    ] {
        assert!(text.contains(needle), "missing {needle:?}:\n{text}");
    }

    let testnet_line = text.lines().find(|l| l.contains("chain 943")).unwrap();
    assert!(
        testnet_line.contains('*'),
        "active network must be marked:\n{text}"
    );
    let mainnet_line = text.lines().find(|l| l.contains("chain 369")).unwrap();
    assert!(
        !mainnet_line.contains('*'),
        "inactive network must not be marked:\n{text}"
    );
}

/// Enter on a highlighted network switches to it: the wallet's active network
/// changes, the render moves the marker, a status line confirms, and the
/// `chainChanged` event fires with the new chain id.
#[test]
fn settings_view_enter_switches_network_and_publishes_event() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut rx = events.subscribe();
    let mut view = SettingsView::new(0); // highlight PulseChain Mainnet

    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    assert_eq!(wallet.networks().active_id(), "pulsechain");

    let text = render(&view, &wallet);
    assert!(
        text.lines()
            .find(|l| l.contains("chain 369"))
            .unwrap()
            .contains('*'),
        "marker must move to the new network:\n{text}"
    );
    assert!(
        !text
            .lines()
            .find(|l| l.contains("chain 943"))
            .unwrap()
            .contains('*'),
        "old network must lose its marker:\n{text}"
    );
    assert!(
        text.contains("Switched to PulseChain Mainnet."),
        "status must confirm the switch:\n{text}"
    );

    // Connected dApps are notified with the new chain id.
    let notification = rx.try_recv().expect("chainChanged event must fire");
    let value: Value = serde_json::from_str(&notification).unwrap();
    assert_eq!(value["method"], "chainChanged");
    assert_eq!(value["params"], "0x171");
}

/// Arrow keys move the highlight; Enter then switches to the highlighted
/// network (0 → 1 → 2 → 1 lands on the testnet).
#[test]
fn settings_view_arrows_move_selection_then_switch() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = SettingsView::new(0);

    view.handle_key(key(KeyCode::Down), &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Down), &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Up), &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    // Selection: 0 → 1 → 2 → 1 = pulsechain-testnet-v4.
    assert_eq!(wallet.networks().active_id(), "pulsechain-testnet-v4");
}

/// Esc returns to the dashboard.
#[test]
fn settings_view_esc_navigates_to_dashboard() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = SettingsView::new(0);

    let outcome = view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Navigate(Screen::Dashboard)));
}
