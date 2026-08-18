//! Tests for the TUI's receive view against a real wallet (anvil-backed).
//!
//! The receive view is display-only: it renders the active address and
//! network. Rendered headlessly via ratatui's `TestBackend` (see
//! `tests/common/mod.rs`).

mod common;

use common::{funded_wallet, render_frame, Anvil};
use crossterm::event::{KeyCode, KeyEvent};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use vaughan_tui::app::{KeyOutcome, Screen};
use vaughan_tui::views::ReceiveView;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

fn render(view: &ReceiveView, wallet: &WalletState) -> String {
    render_frame(100, 24, |f| view.render(f, f.area(), wallet))
}

fn runtime_handle() -> (tokio::runtime::Runtime, Handle) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    (rt, handle)
}

/// The active address and network name are shown, with the receive hint.
#[test]
fn receive_view_renders_active_address_and_network() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let address = wallet.active_address().unwrap().to_string();

    let text = render(&ReceiveView, &wallet);
    assert!(
        text.to_lowercase().contains(&address.to_lowercase()),
        "must render the active address:\n{text}"
    );
    assert!(
        text.contains("PulseChain Testnet V4"),
        "must render the active network:\n{text}"
    );
    assert!(
        text.contains("Your address:"),
        "must label the address:\n{text}"
    );
}

/// A locked wallet renders a placeholder instead of leaking the address.
#[test]
fn receive_view_locked_shows_placeholder() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    wallet.lock();

    let text = render(&ReceiveView, &wallet);
    assert!(
        text.contains("(locked)"),
        "locked wallet must not leak the address:\n{text}"
    );
}

/// The view reads the live wallet: switching networks updates the render.
#[test]
fn receive_view_reflects_network_switch() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);

    assert!(render(&ReceiveView, &wallet).contains("PulseChain Testnet V4"));

    wallet.set_active_network("pulsechain").unwrap();
    let text = render(&ReceiveView, &wallet);
    assert!(
        text.contains("PulseChain Mainnet"),
        "network switch must be reflected:\n{text}"
    );
    assert!(!text.contains("Testnet V4"));
}

/// Esc returns to the dashboard.
#[test]
fn receive_view_esc_navigates_to_dashboard() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = ReceiveView;

    let outcome = view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Navigate(Screen::Dashboard)));
}
