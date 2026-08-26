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

/// Home shows the send-to and amount fields with F4 / F5 chrome keys.
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
        text.contains("Amount"),
        "home must show amount field:\n{text}"
    );
    assert!(
        text.contains("F4"),
        "home must label Send to with F4:\n{text}"
    );
    assert!(
        text.contains("F5"),
        "home must label Amount with F5:\n{text}"
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

/// F4 / F5 focus Send to / Amount from idle home.
#[test]
fn dashboard_view_f4_f5_focus_fields() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let events = EventBus::new();
    let mut view = DashboardView::loading();

    assert!(matches!(
        view.handle_key(key(KeyCode::F(4)), &mut wallet, &handle, &events),
        KeyOutcome::Consumed
    ));
    assert!(matches!(
        view.handle_key(key(KeyCode::Char('0')), &mut wallet, &handle, &events),
        KeyOutcome::Consumed
    ));
    let after_f4 = render(&view, &wallet);
    assert!(
        after_f4.contains('0'),
        "F4 must focus Send to so typing lands there:\n{after_f4}"
    );

    assert!(matches!(
        view.handle_key(key(KeyCode::F(5)), &mut wallet, &handle, &events),
        KeyOutcome::Consumed
    ));
    assert!(matches!(
        view.handle_key(key(KeyCode::Char('9')), &mut wallet, &handle, &events),
        KeyOutcome::Consumed
    ));
    let after_f5 = render(&view, &wallet);
    assert!(
        after_f5.contains('9'),
        "F5 must focus Amount so typing lands there:\n{after_f5}"
    );
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
