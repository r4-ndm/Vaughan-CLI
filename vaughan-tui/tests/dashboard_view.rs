//! Tests for the TUI home (dashboard) send view.

mod common;

use common::{funded_wallet, render_frame, Anvil};
use crossterm::event::{KeyCode, KeyEvent};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use vaughan_tui::app::KeyOutcome;
use vaughan_tui::views::DashboardView;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

fn render(view: &DashboardView, wallet: &WalletState) -> String {
    render_frame(100, 30, |f| view.render(f, f.area(), wallet))
}

fn runtime_handle() -> (tokio::runtime::Runtime, Handle) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    (rt, handle)
}

/// Home shows the send-to field and F1/F2/F3 hint line.
#[test]
fn dashboard_view_shows_send_to() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);

    let text = render(&DashboardView::loading(), &wallet);
    assert!(
        text.contains("Send to"),
        "home must show Send to field:\n{text}"
    );
    assert!(
        text.contains("F1") && text.contains("F2") && text.contains("F3"),
        "home must hint chrome F1/F2/F3:\n{text}"
    );
}

/// Idle home defers footer letters to global handling.
#[test]
fn dashboard_view_idle_defers_footer_letters() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let events = EventBus::new();
    let mut view = DashboardView::loading();

    for c in ['r', 'l', 'v', 'a', 'd'] {
        assert!(
            matches!(
                view.handle_key(key(KeyCode::Char(c)), &mut wallet, &handle, &events),
                KeyOutcome::NotHandled
            ),
            "idle home must defer '{c}' to global shortcuts"
        );
    }
    assert!(wallet.is_unlocked(), "lock is global — view must not lock");
}

/// Enter focuses Send to; hex starts typing immediately.
#[test]
fn dashboard_view_enter_edits_send_to() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let events = EventBus::new();
    let mut view = DashboardView::loading();

    assert!(matches!(
        view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events),
        KeyOutcome::Consumed
    ));
    assert!(matches!(
        view.handle_key(key(KeyCode::Char('0')), &mut wallet, &handle, &events),
        KeyOutcome::Consumed
    ));
}
