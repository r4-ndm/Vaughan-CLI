//! Tests for the TUI's onboarding (create / restore wallet) view.
//!
//! Drives the real `OnboardingView` state machine (Choose → ShowMnemonic /
//! EnterMnemonic → SetPassword → ConfirmPassword) with real key events,
//! renders it headlessly via ratatui's `TestBackend`, and verifies the wallet
//! is actually created on disk. Wallet creation is fully offline, so no anvil
//! is needed here.

mod common;

use std::process::Command;

use common::{fresh_wallet, render_frame};
use crossterm::event::{KeyCode, KeyEvent};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use vaughan_tui::app::{KeyOutcome, Screen};
use vaughan_tui::views::OnboardingView;

/// Passes the password policy (>= 12 chars, upper, lower, digit, symbol).
const STRONG_PASSWORD: &str = "BombProof123!";
/// The canonical BIP-39 test vector — a valid 12-word recovery phrase.
const TEST_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

fn render(view: &OnboardingView, wallet: &WalletState) -> String {
    render_frame(100, 30, |f| view.render(f, f.area(), wallet))
}

fn runtime_handle() -> (tokio::runtime::Runtime, Handle) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    (rt, handle)
}

fn type_text(
    view: &mut OnboardingView,
    text: &str,
    wallet: &mut WalletState,
    handle: &Handle,
    events: &EventBus,
) {
    for c in text.chars() {
        view.handle_key(key(KeyCode::Char(c)), wallet, handle, events);
    }
}

/// The full create flow: `c` shows a fresh 12-word mnemonic, then password +
/// confirmation creates a persisted, unlocked wallet and lands on the
/// dashboard.
#[test]
fn onboarding_create_flow_generates_mnemonic_and_creates_wallet() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = fresh_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = OnboardingView::default();

    // Choose → create.
    assert!(render(&view, &wallet).contains("c — create a new wallet"));
    view.handle_key(key(KeyCode::Char('c')), &mut wallet, &handle, &events);

    // A 12-word recovery phrase is shown (it may wrap across buffer lines).
    let text = render(&view, &wallet);
    assert!(text.contains("Your recovery phrase."), "{text}");
    let words: Vec<&str> = text
        .lines()
        .skip_while(|l| !l.contains("Write it down"))
        .skip(1)
        .take_while(|l| !l.contains("Press Enter"))
        .flat_map(|l| l.split_whitespace())
        // Strip block-border artifacts (│) that attach to edge words.
        .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphabetic()))
        .filter(|w| !w.is_empty())
        .collect();
    assert_eq!(words.len(), 12, "must render a 12-word phrase, got {words:?}:\n{text}");

    // Enter → set password.
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(render(&view, &wallet).contains("Choose a password"));

    type_text(&mut view, STRONG_PASSWORD, &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(render(&view, &wallet).contains("Confirm password"));

    type_text(&mut view, STRONG_PASSWORD, &mut wallet, &handle, &events);
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    assert!(matches!(outcome, KeyOutcome::Navigate(Screen::Dashboard)));
    assert!(wallet.is_initialized(), "wallet must be persisted to disk");
    assert!(wallet.is_unlocked(), "wallet must be unlocked after onboarding");
}

/// Mismatched password confirmation goes back to the password stage with an
/// error and never creates the wallet.
#[test]
fn onboarding_password_mismatch_returns_to_set_password() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = fresh_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = OnboardingView::default();

    view.handle_key(key(KeyCode::Char('c')), &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    type_text(&mut view, STRONG_PASSWORD, &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    type_text(&mut view, "Different123!", &mut wallet, &handle, &events);
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    assert!(!matches!(outcome, KeyOutcome::Navigate(_)), "must not navigate");
    let text = render(&view, &wallet);
    assert!(text.contains("Passwords do not match."), "{text}");
    assert!(text.contains("Choose a password"), "back at the password stage:\n{text}");
    assert!(!wallet.is_initialized(), "wallet must not be created");
}

/// A weak password is rejected by the policy and the flow stays put.
#[test]
fn onboarding_weak_password_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = fresh_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = OnboardingView::default();

    view.handle_key(key(KeyCode::Char('c')), &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    type_text(&mut view, "short", &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    let text = render(&view, &wallet);
    assert!(text.contains("Choose a password"), "still on the password stage:\n{text}");
    assert!(!text.contains("Confirm password"), "must not advance:\n{text}");
    assert!(!wallet.is_initialized());
}

/// Restore with a phrase that is not valid BIP-39 shows an error and stays on
/// the phrase entry stage.
#[test]
fn onboarding_invalid_mnemonic_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = fresh_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = OnboardingView::default();

    view.handle_key(key(KeyCode::Char('r')), &mut wallet, &handle, &events);
    type_text(
        &mut view,
        "this is not a valid recovery phrase",
        &mut wallet,
        &handle,
        &events,
    );
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    let text = render(&view, &wallet);
    assert!(
        text.contains("Enter your 12-word recovery phrase"),
        "must stay on phrase entry:\n{text}"
    );
    assert!(!wallet.is_initialized());
}

/// The restore flow creates the wallet from the entered phrase; the active
/// address matches the canonical derivation of that mnemonic.
#[test]
fn onboarding_restore_flow_creates_wallet_from_phrase() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = fresh_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = OnboardingView::default();

    view.handle_key(key(KeyCode::Char('r')), &mut wallet, &handle, &events);
    type_text(&mut view, TEST_MNEMONIC, &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(render(&view, &wallet).contains("Choose a password"));

    type_text(&mut view, STRONG_PASSWORD, &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    type_text(&mut view, STRONG_PASSWORD, &mut wallet, &handle, &events);
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    assert!(matches!(outcome, KeyOutcome::Navigate(Screen::Dashboard)));
    assert!(wallet.is_initialized() && wallet.is_unlocked());

    // The restored wallet's active account is the canonical derivation of the
    // entered mnemonic (foundry reference).
    let out = Command::new("cast")
        .args(["wallet", "address", "--mnemonic", TEST_MNEMONIC])
        .output()
        .expect("cast must be available");
    let expected = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(
        wallet.active_address().unwrap().to_lowercase(),
        expected.to_lowercase(),
        "restored wallet must derive the active account from the phrase"
    );
}
