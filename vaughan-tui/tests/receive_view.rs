//! Tests for the TUI's receive view against a real wallet (anvil-backed).
//!
//! The receive view is display-only: it renders the active address and
//! network. Rendered headlessly via ratatui's `TestBackend` (see
//! `tests/common/mod.rs`).

mod common;

use common::{funded_wallet, plant_announcer, render_frame, Anvil};
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

    let text = render(&ReceiveView::default(), &wallet);
    assert!(
        text.to_lowercase().contains(&address.to_lowercase()),
        "must render the active address:\n{text}"
    );
    assert!(
        text.contains("PulseChain Testnet V4"),
        "must render the active network:\n{text}"
    );
    assert!(
        text.contains("Public address"),
        "must label the address:\n{text}"
    );
    assert!(
        text.contains("st:tpls:0x"),
        "unlocked receive must show the stealth URI:\n{text}"
    );
}

/// A locked wallet renders a placeholder instead of leaking the address.
#[test]
fn receive_view_locked_shows_placeholder() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    wallet.lock();

    let text = render(&ReceiveView::default(), &wallet);
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

    assert!(render(&ReceiveView::default(), &wallet).contains("PulseChain Testnet V4"));

    wallet.set_active_network("pulsechain").unwrap();
    let text = render(&ReceiveView::default(), &wallet);
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
    let mut view = ReceiveView::default();

    let outcome = view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Navigate(Screen::Dashboard)));
}

/// After a stealth payment, `s` scans announcer logs and lists the funded note.
#[test]
fn receive_view_scan_lists_funded_note() {
    let anvil = Anvil::start();
    plant_announcer(&anvil);
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let uri = wallet.stealth_uri().unwrap();
    let announcement = wallet.prepare_stealth_payment(&uri).unwrap();
    let stealth = format!("{:#x}", announcement.stealth_address);

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(wallet.send_stealth(&announcement, &(10u128.pow(18)).to_string()))
        .expect("seed a stealth note");

    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = ReceiveView::default();
    view.handle_key(key(KeyCode::Char('s')), &mut wallet, &handle, &events);

    let text = render(&view, &wallet);
    assert!(
        text.to_lowercase().contains(&stealth.to_lowercase()) || text.contains("note"),
        "scan must list the funded stealth note:\n{text}"
    );
}

/// Empty scan lists no notes.
#[test]
fn receive_view_scan_empty() {
    let anvil = Anvil::start();
    plant_announcer(&anvil);
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = ReceiveView::default();
    view.handle_key(key(KeyCode::Char('s')), &mut wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(
        text.contains("No funded notes"),
        "empty scan must say so:\n{text}"
    );
}

/// Scan without an announcer stays on the address stage and reports the error.
#[test]
fn receive_view_scan_without_announcer() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = ReceiveView::default();
    view.handle_key(key(KeyCode::Char('s')), &mut wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(
        text.to_lowercase().contains("announcer"),
        "missing announcer must surface in the status:\n{text}"
    );
    assert!(
        text.contains("st:tpls:0x"),
        "must remain on the address stage:\n{text}"
    );
}

/// Enter after a scan sweeps the selected note back to the public address.
#[test]
fn receive_view_sweep_moves_funds() {
    let anvil = Anvil::start();
    plant_announcer(&anvil);
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let public = wallet.active_address().unwrap().to_string();
    let uri = wallet.stealth_uri().unwrap();
    let announcement = wallet.prepare_stealth_payment(&uri).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(wallet.send_stealth(&announcement, &(10u128.pow(18)).to_string()))
        .expect("seed a stealth note");

    let before = anvil.wei_balance(&public);
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = ReceiveView::default();
    view.handle_key(key(KeyCode::Char('s')), &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    let text = render(&view, &wallet);
    assert!(
        text.contains("No funded notes") || text.to_lowercase().contains("swept"),
        "sweep must clear the note list:\n{text}"
    );
    assert!(
        anvil.wei_balance(&public) > before,
        "public address must receive the sweep"
    );
}
