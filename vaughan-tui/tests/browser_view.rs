//! Headless TestBackend integration tests for the Contract Browser REPL view (`BrowserView`).

mod common;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;
use tempfile::tempdir;
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use vaughan_tui::app::{KeyOutcome, Screen};
use vaughan_tui::views::BrowserView;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn runtime_handle() -> (tokio::runtime::Runtime, Handle) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    (rt, handle)
}

fn type_text(
    view: &mut BrowserView,
    text: &str,
    wallet: &mut WalletState,
    handle: &Handle,
    events: &EventBus,
) {
    for c in text.chars() {
        view.handle_key(key(KeyCode::Char(c)), wallet, handle, events);
    }
}

#[test]
fn browser_view_renders_initial_banner() {
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

#[test]
fn browser_view_help_command() {
    let tmp = tempdir().unwrap();
    let mut wallet = common::fresh_wallet(tmp.path());
    let mut view = BrowserView::default();
    let events = EventBus::new();
    let (_rt, handle) = runtime_handle();

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

#[test]
fn browser_view_esc_navigates_to_dashboard() {
    let tmp = tempdir().unwrap();
    let mut wallet = common::fresh_wallet(tmp.path());
    let mut view = BrowserView::default();
    let events = EventBus::new();
    let (_rt, handle) = runtime_handle();

    // Esc on empty input navigates to Dashboard
    let outcome = view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);
    assert_eq!(outcome, KeyOutcome::Navigate(Screen::Dashboard));

    // Esc with text in buffer clears text, doesn't navigate
    type_text(&mut view, "browse 0x123", &mut wallet, &handle, &events);
    let outcome2 = view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);
    assert_eq!(outcome2, KeyOutcome::Consumed);
}

#[test]
fn browser_view_command_history_navigation() {
    let tmp = tempdir().unwrap();
    let mut wallet = common::fresh_wallet(tmp.path());
    let mut view = BrowserView::default();
    let events = EventBus::new();
    let (_rt, handle) = runtime_handle();

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

#[test]
fn browser_view_repl_interactive_session_with_anvil() {
    let anvil = common::Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = common::funded_wallet(dir.path(), &anvil);
    let events = EventBus::new();
    let (_rt, handle) = runtime_handle();
    let mut view = BrowserView::default();

    // 1. Browse account #0 on anvil
    let target = wallet.active_address().unwrap().to_string();
    type_text(
        &mut view,
        &format!("browse {target}"),
        &mut wallet,
        &handle,
        &events,
    );
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    let text1 = common::render_frame(100, 30, |frame| view.render(frame, frame.area(), &wallet));
    assert!(
        text1.contains("Loaded contract:"),
        "must confirm contract load in terminal:\n{text1}"
    );

    // 2. Query info
    type_text(&mut view, "info", &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    let text2 = common::render_frame(100, 30, |frame| view.render(frame, frame.area(), &wallet));
    assert!(
        text2.contains("Fingerprint:"),
        "must display fingerprint info:\n{text2}"
    );

    // 3. Clear console
    type_text(&mut view, "clear", &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    let text3 = common::render_frame(100, 30, |frame| view.render(frame, frame.area(), &wallet));
    assert!(
        !text3.contains("Loaded contract:"),
        "console must be cleared:\n{text3}"
    );
}

/// `callraw` against a planted runtime that returns `0x2a`.
#[test]
fn browser_view_callraw_against_planted_contract() {
    let anvil = common::Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = common::funded_wallet(dir.path(), &anvil);
    let events = EventBus::new();
    let (_rt, handle) = runtime_handle();
    let mut view = BrowserView::default();

    const CONTRACT: &str = "0x1111111111111111111111111111111111111111";
    anvil
        .rpc("anvil_setCode", json!([CONTRACT, "0x602a60005260206000f3"]))
        .expect("anvil_setCode");

    type_text(
        &mut view,
        &format!("browse {CONTRACT}"),
        &mut wallet,
        &handle,
        &events,
    );
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    type_text(&mut view, "callraw 0x", &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    let text = common::render_frame(100, 30, |frame| view.render(frame, frame.area(), &wallet));
    assert!(
        text.contains("Raw Output"),
        "must show raw call output:\n{text}"
    );
    assert!(
        text.to_lowercase().contains("2a"),
        "planted runtime returns 0x2a:\n{text}"
    );
}
