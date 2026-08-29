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
use vaughan_provider::{EventBus, ProviderEvent};
use vaughan_tui::app::KeyOutcome;
use vaughan_tui::jobs::UiJob;
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

/// Extract the unlock job a password submit should have produced.
fn expect_unlock_job(outcome: KeyOutcome) -> UiJob {
    match outcome {
        KeyOutcome::StartJob(job @ UiJob::Unlock { .. }) => job,
        other => panic!("expected StartJob(UiJob::Unlock), got {other:?}"),
    }
}

/// Simulate the app side of the async unlock (`spawn_job` + `poll_jobs`):
/// run the KDF off the wallet lock, apply accounts + mode, publish
/// `accountsChanged` — exactly what `App::poll_jobs` does on success.
fn drive_unlock_success(job: UiJob, wallet: &mut WalletState, events: &EventBus) {
    let UiJob::Unlock { password, mode } = job else {
        unreachable!("checked by expect_unlock_job")
    };
    let payload = wallet.unlock_payload().unwrap();
    let accounts = payload
        .decrypt(&password)
        .unwrap_or_else(|e| panic!("unlock job failed: {}", e.user_message()));
    wallet.apply_unlocked_accounts(accounts);
    wallet.set_operating_mode(mode);
    let addr = wallet.active_address().unwrap().to_string();
    events.publish(ProviderEvent::AccountsChanged(vec![addr]));
}

/// Correct password: submit spawns the off-thread unlock job (spinner shows,
/// wallet still locked), and completing it unlocks + publishes
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
    let job =
        expect_unlock_job(view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events));

    // While the KDF runs, the wallet is still locked and the spinner shows.
    assert!(!wallet.is_unlocked(), "job in flight — still locked");
    let text = render(&view, &wallet);
    assert!(text.contains("Unlocking"), "spinner status:\n{text}");

    // Keys are swallowed mid-unlock (no double submit, no Esc).
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Consumed));

    drive_unlock_success(job, &mut wallet, &events);

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

/// A wrong password fails the job; the app routes the error back to the view,
/// the wallet stays locked, and no event fires.
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
    let job =
        expect_unlock_job(view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events));

    // The job's KDF fails; the app reports it via `unlock_failed`.
    let UiJob::Unlock { password, .. } = job else {
        unreachable!()
    };
    let payload = wallet.unlock_payload().unwrap();
    let err = payload
        .decrypt(&password)
        .err()
        .expect("wrong password must fail");
    view.unlock_failed(err.user_message());

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

    // The field accepts input again after the failure.
    type_text(&mut view, "x", &mut wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(text.contains('*'), "typing works again:\n{text}");
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

/// The picker opens on the Human / Sentient role choice; an uncreated
/// sentient wallet is flagged as new.
#[test]
fn picker_lists_human_or_sentient_roles() {
    let dir = tempfile::tempdir().unwrap();
    let wallet = fresh_wallet(dir.path());
    let view = UnlockView::with_profiles(
        vec![meta(DEFAULT_PROFILE, true), meta(SENTIENT_PROFILE, false)],
        DEFAULT_PROFILE,
    );

    let text = render(&view, &wallet);
    assert!(text.contains("Human"), "human row:\n{text}");
    assert!(text.contains("Sentient"), "sentient row:\n{text}");
    assert!(
        text.contains("new — vault created next"),
        "uninitialized tag:\n{text}"
    );
}

/// Enter on Sentient skips the mode step (auto-exec only runs on the agent
/// wallet's seed) and emits `SwitchProfile` with SentientTrader.
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
        matches!(
            outcome,
            KeyOutcome::SwitchProfile(ref name, OperatingMode::SentientTrader)
            if name == SENTIENT_PROFILE
        ),
        "expected SwitchProfile(sentient, SentientTrader), got {outcome:?}"
    );

    let text = render(&view, &wallet);
    assert!(
        text.contains("Password"),
        "password stage after pick:\n{text}"
    );

    // Esc from the password stage returns to the role choice.
    let outcome = view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Consumed));
    let text = render(&view, &wallet);
    assert!(text.contains("Human"), "role step after Esc:\n{text}");
}

/// Human with several wallets lists them; picking one advances to the mode
/// step, and Enter there emits `SwitchProfile` with the chosen mode.
#[test]
fn picker_wallet_then_mode_for_regular_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = fresh_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = UnlockView::with_profiles(
        vec![meta(DEFAULT_PROFILE, true), meta("savings", true)],
        DEFAULT_PROFILE,
    );

    // Role step: Human is pre-selected — Enter to list the human wallets.
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Consumed));
    let text = render(&view, &wallet);
    assert!(text.contains("default"), "default wallet row:\n{text}");
    assert!(text.contains("savings"), "savings wallet row:\n{text}");

    // Wallet step: move to `savings`, Enter → mode step (no switch yet).
    let outcome = view.handle_key(key(KeyCode::Down), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Consumed));
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(
        matches!(outcome, KeyOutcome::Consumed),
        "wallet pick should not switch before mode is chosen, got {outcome:?}"
    );

    let text = render(&view, &wallet);
    assert!(
        text.contains("Mode for savings"),
        "mode step for savings:\n{text}"
    );
    assert!(
        text.contains("Human only — manual wallet, no agent"),
        "human-only row:\n{text}"
    );
    assert!(
        text.contains("Advisor — agent proposes, you approve"),
        "advisor row:\n{text}"
    );
    assert!(
        !text.contains("Sentient"),
        "sentient is never offered on a regular wallet:\n{text}"
    );

    // Advisor is pre-selected; Enter emits the switch with AiAssisted.
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(
        matches!(
            outcome,
            KeyOutcome::SwitchProfile(ref name, OperatingMode::AiAssisted)
            if name == "savings"
        ),
        "expected SwitchProfile(savings, AiAssisted), got {outcome:?}"
    );

    // Esc from password returns to the mode step.
    let outcome = view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Consumed));
    let text = render(&view, &wallet);
    assert!(
        text.contains("Mode for savings"),
        "mode step after Esc:\n{text}"
    );
}

/// A single human wallet skips the wallet list: Human on the role step goes
/// straight to the Human only / Advisor choice.
#[test]
fn single_profile_offers_human_and_advisor_rows() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = fresh_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = UnlockView::with_profiles(vec![meta(DEFAULT_PROFILE, true)], DEFAULT_PROFILE);

    let text = render(&view, &wallet);
    assert!(text.contains("Human"), "role step:\n{text}");

    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Consumed));

    let text = render(&view, &wallet);
    assert!(
        text.contains("Human only — manual wallet, no agent"),
        "human-only row:\n{text}"
    );
    assert!(
        text.contains("Advisor — agent proposes, you approve"),
        "advisor row:\n{text}"
    );
    assert!(
        !text.contains("Sentient — agent auto-exec"),
        "sentient mode is never offered on a regular wallet:\n{text}"
    );
}

/// Unlocking via the Advisor row locks in AiAssisted for the session.
#[test]
fn unlock_via_advisor_row_sets_ai_assisted_mode() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = locked_wallet(&anvil, dir.path());

    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = UnlockView::with_profiles(vec![meta(DEFAULT_PROFILE, true)], DEFAULT_PROFILE);

    // Human (pre-selected) → Advisor (pre-selected) — Enter through.
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Consumed));
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(
        matches!(
            outcome,
            KeyOutcome::SwitchProfile(ref name, OperatingMode::AiAssisted)
            if name == DEFAULT_PROFILE
        ),
        "expected SwitchProfile(default, AiAssisted), got {outcome:?}"
    );

    type_text(&mut view, PASSWORD, &mut wallet, &handle, &events);
    let job =
        expect_unlock_job(view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events));
    drive_unlock_success(job, &mut wallet, &events);
    assert!(wallet.is_unlocked());
    assert_eq!(wallet.operating_mode(), OperatingMode::AiAssisted);
}

/// Unlocking via the Human only row locks in HumanOnly for the session.
#[test]
fn unlock_via_human_only_row_sets_human_mode() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = locked_wallet(&anvil, dir.path());

    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = UnlockView::with_profiles(vec![meta(DEFAULT_PROFILE, true)], DEFAULT_PROFILE);

    // Human → mode step, then up from Advisor to Human only.
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Consumed));
    let outcome = view.handle_key(key(KeyCode::Up), &mut wallet, &handle, &events);
    assert!(matches!(outcome, KeyOutcome::Consumed));
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(
        matches!(
            outcome,
            KeyOutcome::SwitchProfile(ref name, OperatingMode::HumanOnly)
            if name == DEFAULT_PROFILE
        ),
        "expected SwitchProfile(default, HumanOnly), got {outcome:?}"
    );

    type_text(&mut view, PASSWORD, &mut wallet, &handle, &events);
    let job =
        expect_unlock_job(view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events));
    drive_unlock_success(job, &mut wallet, &events);
    assert!(wallet.is_unlocked());
    assert_eq!(wallet.operating_mode(), OperatingMode::HumanOnly);
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
    let job =
        expect_unlock_job(view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events));
    drive_unlock_success(job, &mut wallet, &events);

    assert!(wallet.is_unlocked());
    assert_eq!(wallet.operating_mode(), OperatingMode::SentientTrader);
}

/// End-to-end picker flow for Sentient with a real vault on disk: role pick
/// → `SwitchProfile` → the app reloads the sentient vault → password unlocks
/// it and lands on the dashboard in SentientTrader mode.
#[test]
fn sentient_picker_password_unlocks_real_vault() {
    let dir = tempfile::tempdir().unwrap();
    let _ = funded_wallet_at(dir.path(), "http://127.0.0.1:8545");

    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut wallet = fresh_wallet(tempfile::tempdir().unwrap().path());
    let mut view = UnlockView::with_profiles(
        vec![meta(DEFAULT_PROFILE, true), meta(SENTIENT_PROFILE, true)],
        DEFAULT_PROFILE,
    );

    // Role step: Sentient is row 2.
    view.handle_key(key(KeyCode::Down), &mut wallet, &handle, &events);
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(
        matches!(
            outcome,
            KeyOutcome::SwitchProfile(ref name, OperatingMode::SentientTrader)
            if name == SENTIENT_PROFILE
        ),
        "expected SwitchProfile(sentient, SentientTrader), got {outcome:?}"
    );

    // The app answers SwitchProfile by loading the sentient vault.
    let mut wallet = WalletState::load_with_session(
        dir.path().join("wallet.json"),
        OperatingMode::HumanOnly,
        SENTIENT_PROFILE,
    )
    .unwrap();

    type_text(&mut view, PASSWORD, &mut wallet, &handle, &events);
    let job =
        expect_unlock_job(view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events));
    drive_unlock_success(job, &mut wallet, &events);
    assert!(wallet.is_unlocked());
    assert_eq!(wallet.operating_mode(), OperatingMode::SentientTrader);
}
