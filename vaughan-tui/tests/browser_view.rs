//! Headless TestBackend integration tests for the Contract Browser REPL view (`BrowserView`).

mod common;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempfile::tempdir;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use vaughan_tui::app::{KeyOutcome, Screen};
use vaughan_tui::views::BrowserView;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_text(
    view: &mut BrowserView,
    text: &str,
    wallet: &mut WalletState,
    handle: &tokio::runtime::Handle,
    events: &EventBus,
) {
    for c in text.chars() {
        view.handle_key(key(KeyCode::Char(c)), wallet, handle, events);
    }
}

#[tokio::test]
async fn browser_view_renders_initial_banner() {
    let tmp = tempdir().unwrap();
    let wallet = common::fresh_wallet(tmp.path());
    let view = BrowserView::default();
    let text = common::render_frame(100, 30, |frame| view.render(frame, frame.area(), &wallet));

    assert!(
        text.contains("Vaughan Contract Browser"),
        "must show title banner:\n{text}"
    );
    assert!(
        text.contains("wiz4rd-engine"),
        "must mention wiz4rd-engine:\n{text}"
    );
    assert!(
        text.contains("Target:"),
        "must show target context:\n{text}"
    );
}

#[tokio::test]
async fn browser_view_help_command() {
    let tmp = tempdir().unwrap();
    let mut wallet = common::fresh_wallet(tmp.path());
    let mut view = BrowserView::default();
    let events = EventBus::new();
    let handle = tokio::runtime::Handle::current();

    type_text(&mut view, "help", &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    let text = common::render_frame(100, 30, |frame| view.render(frame, frame.area(), &wallet));

    assert!(
        text.contains("Available Commands:"),
        "must show help output:\n{text}"
    );
    assert!(
        text.contains("browse <0xAddress>"),
        "must list browse command:\n{text}"
    );
    assert!(
        text.contains("call <fn_name>"),
        "must list call command:\n{text}"
    );
}

#[tokio::test]
async fn browser_view_esc_navigates_to_dashboard() {
    let tmp = tempdir().unwrap();
    let mut wallet = common::fresh_wallet(tmp.path());
    let mut view = BrowserView::default();
    let events = EventBus::new();
    let handle = tokio::runtime::Handle::current();

    // Esc on empty input navigates to Dashboard
    let outcome = view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);
    assert_eq!(outcome, KeyOutcome::Navigate(Screen::Dashboard));

    // Esc with text in buffer clears text, doesn't navigate
    type_text(&mut view, "browse 0x123", &mut wallet, &handle, &events);
    let outcome2 = view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);
    assert_eq!(outcome2, KeyOutcome::Consumed);
}

#[tokio::test]
async fn browser_view_command_history_navigation() {
    let tmp = tempdir().unwrap();
    let mut wallet = common::fresh_wallet(tmp.path());
    let mut view = BrowserView::default();
    let events = EventBus::new();
    let handle = tokio::runtime::Handle::current();

    type_text(&mut view, "help", &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    type_text(&mut view, "clear", &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    // Up arrow recalls "clear"
    view.handle_key(key(KeyCode::Up), &mut wallet, &handle, &events);
    let text = common::render_frame(100, 30, |frame| view.render(frame, frame.area(), &wallet));
    assert!(
        text.contains("clear"),
        "must recall clear from history:\n{text}"
    );

    // Up arrow again recalls "help"
    view.handle_key(key(KeyCode::Up), &mut wallet, &handle, &events);
    let text2 = common::render_frame(100, 30, |frame| view.render(frame, frame.area(), &wallet));
    assert!(
        text2.contains("help"),
        "must recall help from history:\n{text2}"
    );
}
