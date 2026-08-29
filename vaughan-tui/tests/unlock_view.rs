//! Tests for the TUI's unlock view against a real wallet (anvil-backed).
//!
//! Drives the real `UnlockView` with real key events and renders it headlessly
//! via ratatui's `TestBackend`. Unlocking decrypts the vault offline; the
//! wallet itself is funded from the anvil dev mnemonic so the address is
//! meaningful and the `accountsChanged` event dApps receive can be asserted.

mod common;

use common::{fresh_wallet, funded_wallet, funded_wallet_at, render_frame, Anvil, PASSWORD};
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;
use tokio::runtime::Handle;
use vaughan_core::core::{
    OperatingMode, ProfileMeta, WalletState, DEFAULT_PROFILE, SENTIENT_PROFILE,
};
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

/// Correct password unlocks the vault, navigates to dashboard, and publishes
/// `accountsChanged`.
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
        matches!(outcome, KeyOutcome::Navigate(Screen::Dashboard)),
        "password success should navigate to dashboard, got {outcome:?}"
    );
    assert!(wallet.is_unlocked(), "wallet must be unlocked");
    assert_eq!(wallet.operating_mode(), OperatingMode::HumanOnly);

    let notification = rx.try_recv().expect("accountsChanged event must fire");
    let value: Value = serde_json::from_str(&notification.to_notification()).unwrap();
    assert_eq!(value["method"], "accountsChanged");
    assert_eq!(
        value["params"][0].as_str().unwrap().to_lowercase(),
        wallet.active_address().unwrap().to_string().to_lowercase()
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

// ---------- Profile picker (FR-5.1 mode switch at unlock) ----------

fn meta(name: &str, initialized: bool) -> ProfileMeta {
    ProfileMeta {
        name: name.to_string(),
        path: std::path::PathBuf::from(format!("/nonexistent/{name}/wallet.json")),
        initialized,
        is_sentient: name == SENTIENT_PROFILE,
    }
}

/// Multiple profiles open the picker with per-profile mode badges; the
/// current profile is pre-selected.
#[test]
fn picker_lists_profiles_with_mode_badges() {
    let dir = tempfile::tempdir().unwrap();
    let wallet = fresh_wallet(dir.path());
    let view = UnlockView::with_profiles(
        vec![meta(DEFAULT_PROFILE, true), meta(SENTIENT_PROFILE, false)],
        DEFAULT_PROFILE,
    );

    let text = render(&view, &wallet);
    assert!(
        text.contains("Advisor — you approve"),
        "advisor badge:\n{text}"
    );
    assert!(
        text.contains("Sentient — agent auto-exec"),
        "sentient badge:\n{text}"
    );
    assert!(
        text.contains("new — vault created next"),
        "uninitialized tag:\n{text}"
    );
}

/// Enter on a profile emits `SwitchProfile` for the app to reload the vault;
/// an initialized profile then continues to the password stage.
#[test]
fn picker_enter_on_sentient_emits_switch_profile() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = fresh_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = UnlockView::with_profiles(
        vec![meta(DEFAULT_PROFILE, true), meta(SENTIENT_PROFILE, true)],
        DEFAULT_PROFILE,
    );

    let outcome = view.handle_key(key(KeyCode::Down), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Consumed));
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(
        matches!(outcome, KeyOutcome::SwitchProfile(ref name) if name == SENTIENT_PROFILE),
        "expected SwitchProfile(sentient), got {outcome:?}"
    );

    let text = render(&view, &wallet);
    assert!(
        text.contains("Password"),
        "password stage after pick:\n{text}"
    );

    // Esc from the password stage returns to the picker.
    let outcome = view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Consumed));
    let text = render(&view, &wallet);
    assert!(
        text.contains("picks agent mode"),
        "picker after Esc:\n{text}"
    );
}

/// A single profile keeps the classic password-only screen (no picker).
#[test]
fn single_profile_skips_picker() {
    let dir = tempfile::tempdir().unwrap();
    let wallet = fresh_wallet(dir.path());
    let view = UnlockView::with_profiles(vec![meta(DEFAULT_PROFILE, true)], DEFAULT_PROFILE);

    let text = render(&view, &wallet);
    assert!(text.contains("Password"), "password stage:\n{text}");
    assert!(
        !text.contains("picks agent mode"),
        "no picker box for one profile:\n{text}"
    );
}

/// Unlocking a vault under the sentient profile locks in SentientTrader
/// mode for the session (FR-5.1); the default profile stays HumanOnly
/// (covered by `unlock_view_correct_password_unlocks_and_publishes_event`).
#[test]
fn unlock_sentient_profile_sets_sentient_mode() {
    let dir = tempfile::tempdir().unwrap();
    // Create the vault offline, then reload it as the sentient profile.
    let _ = funded_wallet_at(dir.path(), "http://127.0.0.1:8545");
    let mut wallet = WalletState::load_with_session(
        dir.path().join("wallet.json"),
        OperatingMode::HumanOnly,
        SENTIENT_PROFILE,
    )
    .unwrap();
    assert!(!wallet.is_unlocked(), "fresh load must be locked");

    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = UnlockView::default();

    type_text(&mut view, PASSWORD, &mut wallet, &handle, &events);
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    assert!(
        matches!(outcome, KeyOutcome::Navigate(Screen::Dashboard)),
        "password success should navigate to dashboard, got {outcome:?}"
    );
    assert!(wallet.is_unlocked());
    assert_eq!(wallet.operating_mode(), OperatingMode::SentientTrader);
}
