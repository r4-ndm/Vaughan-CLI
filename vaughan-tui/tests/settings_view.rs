//! Tests for the TUI's settings (network switching) view against a real
//! wallet. Rendered headlessly via ratatui's `TestBackend` (see
//! `tests/common/mod.rs`); network switches are verified both in the wallet
//! state and through the `chainChanged` event dApps would receive.

mod common;

use common::{funded_wallet, render_frame, Anvil};
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;
use tokio::runtime::Handle;
use vaughan_core::core::{OperatingMode, WalletState};
use vaughan_provider::EventBus;
use vaughan_tui::app::KeyOutcome;
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
        "EthereumPoW",
        "chain 10001",
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

/// `h` opens Ledger + Trezor Linux USB help with official udev links.
#[test]
fn settings_view_h_opens_hardware_usb_help() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = SettingsView::new(0);

    view.handle_key(key(KeyCode::Char('h')), &mut wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(text.contains("Add udev rules"));
    assert!(text.contains("support.ledger.com/article/115005165269-zd"));
    assert!(text.contains("trezor.io/guides/trezorctl/udev-rules"));
    assert!(
        !text.contains("github.com"),
        "must not point at GitHub for udev:\n{text}"
    );

    view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(
        text.contains("PulseChain"),
        "Esc returns to network list:\n{text}"
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
    let value: Value = serde_json::from_str(&notification.to_notification()).unwrap();
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
    assert!(matches!(outcome, KeyOutcome::Back));
}

/// CDP control row toggles agent browser control and persists across reload.
#[test]
fn settings_view_enter_toggles_agent_browser_control() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = SettingsView::new(0);
    let net_len = wallet.networks().networks().len();

    assert!(!wallet.agent_browser_control());
    for _ in 0..net_len {
        view.handle_key(key(KeyCode::Down), &mut wallet, &handle, &events);
    }
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(wallet.agent_browser_control());

    let path = wallet.path().to_path_buf();
    let reloaded = WalletState::load_with_session(path, OperatingMode::HumanOnly, "default")
        .expect("reload wallet");
    assert!(reloaded.agent_browser_control());
}

/// Autonomy row toggles Advisor/Operator with Enter.
#[test]
fn settings_view_enter_toggles_autonomy_tier() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = SettingsView::new(0);
    let net_len = wallet.networks().networks().len();

    for _ in 0..=net_len {
        view.handle_key(key(KeyCode::Down), &mut wallet, &handle, &events);
    }
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert_eq!(
        wallet.agent_autonomy_tier(),
        vaughan_core::core::AgentAutonomyTier::Operator
    );
}

/// `r` opens RPC picker; Enter selects a preset and persists.
#[test]
fn settings_view_r_persists_rpc_primary() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = SettingsView::new(0); // PulseChain mainnet

    view.handle_key(key(KeyCode::Char('r')), &mut wallet, &handle, &events);
    let mid = render(&view, &wallet);
    assert!(mid.contains("RPC URL"));
    assert!(mid.contains("PublicNode"));

    view.handle_key(key(KeyCode::Down), &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    assert_eq!(
        wallet.network_rpc_primary("pulsechain"),
        Some("https://pulsechain-rpc.publicnode.com")
    );

    let path = wallet.path().to_path_buf();
    let reloaded = WalletState::load_with_session(path, OperatingMode::HumanOnly, "default")
        .expect("reload wallet");
    assert_eq!(
        reloaded.network_rpc_primary("pulsechain"),
        Some("https://pulsechain-rpc.publicnode.com")
    );
    let (primary, _) = reloaded.rpc_endpoints_for(reloaded.networks().get("pulsechain").unwrap());
    assert_eq!(primary, "https://pulsechain-rpc.publicnode.com");
}

/// `e` on a custom network opens the edit form with current values.
#[test]
fn settings_view_e_opens_custom_edit_form() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    wallet
        .add_custom_network("Anvil", 31_337, "http://127.0.0.1:8545", "ETH", true)
        .unwrap();
    let idx = wallet
        .networks()
        .networks()
        .iter()
        .position(|n| n.id == "custom-31337")
        .unwrap();
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = SettingsView::new(idx);

    view.handle_key(key(KeyCode::Char('e')), &mut wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(text.contains("Edit custom network"));
    assert!(text.contains("31337"));
    assert!(text.contains("Anvil"));
}

fn type_text(
    view: &mut SettingsView,
    text: &str,
    wallet: &mut WalletState,
    handle: &Handle,
    events: &EventBus,
) {
    for c in text.chars() {
        view.handle_key(key(KeyCode::Char(c)), wallet, handle, events);
    }
}

/// `p` opens change-password; full rotate unlocks under the new password only.
#[test]
fn settings_view_p_changes_vault_password() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = SettingsView::new(0);

    view.handle_key(key(KeyCode::Char('p')), &mut wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(
        text.contains("Change vault password"),
        "p opens password flow:\n{text}"
    );

    type_text(&mut view, common::PASSWORD, &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(
        render(&view, &wallet).contains("Choose new password"),
        "after current password:\n{}",
        render(&view, &wallet)
    );

    const NEW: &str = "NewBombProof9!";
    type_text(&mut view, NEW, &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(
        render(&view, &wallet).contains("Confirm new password"),
        "after new password:\n{}",
        render(&view, &wallet)
    );

    type_text(&mut view, NEW, &mut wallet, &handle, &events);
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(
        matches!(
            &outcome,
            vaughan_tui::app::KeyOutcome::Flash(msg)
                if msg.to_lowercase().contains("password changed successfully")
        ),
        "success must chrome-flash, got {outcome:?}"
    );
    let text = render(&view, &wallet);
    assert!(
        text.contains("Vault password updated"),
        "status after rotate:\n{text}"
    );
    assert!(
        text.contains("PulseChain"),
        "returns to network list:\n{text}"
    );
    assert!(
        text.contains("p password"),
        "list footer advertises password:\n{text}"
    );

    wallet.lock();
    assert!(wallet
        .unlock(&secrecy::SecretString::from(common::PASSWORD.to_string()))
        .is_err());
    wallet
        .unlock(&secrecy::SecretString::from(NEW.to_string()))
        .unwrap();
}

/// Esc aborts change-password without touching the vault.
#[test]
fn settings_view_change_password_esc_cancels() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let mut view = SettingsView::new(0);

    view.handle_key(key(KeyCode::Char('p')), &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(text.contains("Password change cancelled"));
    assert!(text.contains("PulseChain"));
}

#[test]
fn settings_view_sentient_shows_policy_shortcut_and_default_refuses_y() {
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();

    // Non-sentient wallet hides `y policy` and refuses `y`.
    let mut default_wallet = WalletState::load_with_session(
        dir.path().join("default-wallet.json"),
        OperatingMode::HumanOnly,
        "default",
    )
    .unwrap();
    let mut view = SettingsView::new(0);
    assert!(!render(&view, &default_wallet).contains("y policy"));
    view.handle_key(
        key(KeyCode::Char('y')),
        &mut default_wallet,
        &handle,
        &events,
    );
    assert!(render(&view, &default_wallet).contains("only available on the sentient profile"));

    // Sentient profile advertises `y policy`.
    let sentient_wallet = WalletState::load_with_session(
        dir.path().join("sentient-wallet.json"),
        OperatingMode::SentientTrader,
        "sentient",
    )
    .unwrap();
    let sentient_view = SettingsView::new(0);
    assert!(render(&sentient_view, &sentient_wallet).contains("y policy"));
}

static SIGNER_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct SignerSocketGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev: Option<String>,
}

impl SignerSocketGuard {
    fn set(path: &std::path::Path) -> Self {
        let lock = SIGNER_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("VAUGHAN_SIGNER_SOCKET").ok();
        std::env::set_var("VAUGHAN_SIGNER_SOCKET", path);
        Self { _lock: lock, prev }
    }
}

impl Drop for SignerSocketGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("VAUGHAN_SIGNER_SOCKET", v),
            None => std::env::remove_var("VAUGHAN_SIGNER_SOCKET"),
        }
    }
}

struct StubSigner {
    _dir: tempfile::TempDir,
    sock_path: std::path::PathBuf,
    requests: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for StubSigner {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn spawn_signer_stub(handle: &Handle, responses: Vec<Value>) -> StubSigner {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("signerd.sock");
    let listener = handle
        .block_on(async { UnixListener::bind(&sock_path) })
        .unwrap();
    let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let rec = requests.clone();
    let task = handle.spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let queue = queue.clone();
            let rec = rec.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                if let Ok(req) = serde_json::from_str::<Value>(line.trim()) {
                    rec.lock().unwrap().push(req);
                }
                let next = queue.lock().unwrap().pop_front().unwrap_or_else(|| {
                    serde_json::json!({"ok": false, "code": "unexpected_call", "message": "stub queue empty"})
                });
                let payload = format!("{next}\n");
                let _ = stream.write_all(payload.as_bytes()).await;
            });
        }
    });
    StubSigner {
        _dir: dir,
        sock_path,
        requests,
        task,
    }
}

fn sentient_wallet(dir: &std::path::Path) -> WalletState {
    WalletState::load_with_session(
        dir.join("sentient-wallet.json"),
        OperatingMode::SentientTrader,
        "sentient",
    )
    .unwrap()
}

fn initial_policy_value() -> Value {
    let p = vaughan_agent::sentient::AgentSessionPolicy {
        max_position_pct: 15,
        ..Default::default()
    };
    serde_json::to_value(&p).unwrap()
}

fn future_unix_secs(offset: u64) -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + offset
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn render_wide(view: &SettingsView, wallet: &WalletState) -> String {
    render_frame(160, 30, |f| view.render(f, f.area(), wallet))
}

fn open_edit_and_stage_42(
    view: &mut SettingsView,
    wallet: &mut WalletState,
    handle: &Handle,
    events: &EventBus,
) {
    view.handle_key(key(KeyCode::Char('y')), wallet, handle, events);
    view.handle_key(key(KeyCode::Down), wallet, handle, events);
    for _ in 0..4 {
        view.handle_key(key(KeyCode::Backspace), wallet, handle, events);
    }
    view.handle_key(key(KeyCode::Char('4')), wallet, handle, events);
    view.handle_key(key(KeyCode::Char('2')), wallet, handle, events);
    view.handle_key(key(KeyCode::Enter), wallet, handle, events);
}

#[test]
fn sentient_policy_submit_opens_root_approval_without_password_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = sentient_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let expires = future_unix_secs(600);
    let stub = spawn_signer_stub(
        &handle,
        vec![
            serde_json::json!({
                "ok": true,
                "policy": initial_policy_value(),
                "policy_digest": "digest_initial_0001"
            }),
            serde_json::json!({
                "ok": false,
                "code": "step_up_required",
                "message": "run: sudo vaughan signerd approve",
                "policy_digest": "digest_staged_abcdef",
                "pending": {
                    "policy_digest": "digest_staged_abcdef",
                    "expires_at_unix": expires,
                    "approval_present": false
                }
            }),
        ],
    );
    let _guard = SignerSocketGuard::set(&stub.sock_path);
    let mut view = SettingsView::new(0);

    open_edit_and_stage_42(&mut view, &mut wallet, &handle, &events);
    let text = render(&view, &wallet);
    let lower = text.to_ascii_lowercase();

    assert!(
        !lower.contains("password"),
        "wait screen must never mention password:\n{text}"
    );
    assert!(
        !lower.contains("vault"),
        "wait screen must not prompt for vault credentials:\n{text}"
    );
    assert!(text.contains("sudo vaughan signerd approve"), "{text}");
    assert!(text.contains("digest_stage"), "{text}");
    assert!(
        text.contains(
            "run it from a clean console (Ctrl+Alt+F3) or run0, not a normal terminal tab"
        ),
        "{text}"
    );
    assert!(text.contains("Expires in "), "{text}");

    let reqs = stub.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1]["op"], "policy_update");
    assert_eq!(reqs[1]["auth"], "stage");
}

#[test]
fn sentient_policy_step_up_pending_keeps_editor_open_with_draft() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = sentient_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let stub = spawn_signer_stub(
        &handle,
        vec![
            serde_json::json!({
                "ok": true,
                "policy": initial_policy_value(),
                "policy_digest": "digest_initial_0001"
            }),
            serde_json::json!({
                "ok": false,
                "code": "step_up_pending",
                "message": "another policy update is already staged"
            }),
        ],
    );
    let _guard = SignerSocketGuard::set(&stub.sock_path);
    let mut view = SettingsView::new(0);

    open_edit_and_stage_42(&mut view, &mut wallet, &handle, &events);
    let text = render_wide(&view, &wallet);
    let expected = vaughan_core::core::policy_step_up::StepUpError::Daemon {
        code: "step_up_pending".into(),
        message: "another policy update is already staged".into(),
    }
    .to_string();

    assert!(
        text.contains("max_position_pct"),
        "must stay on field editor:\n{text}"
    );
    assert!(
        text.contains("42"),
        "edited draft must remain intact:\n{text}"
    );
    assert!(
        normalize_ws(&text).contains(&expected),
        "expected `{expected}` in:\n{text}"
    );
}

#[test]
fn sentient_policy_esc_on_wait_screen_applies_nothing_and_leaves_pending() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = sentient_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let expires = future_unix_secs(600);
    let stub = spawn_signer_stub(
        &handle,
        vec![
            serde_json::json!({
                "ok": true,
                "policy": initial_policy_value(),
                "policy_digest": "digest_initial_0001"
            }),
            serde_json::json!({
                "ok": false,
                "code": "step_up_required",
                "message": "run: sudo vaughan signerd approve",
                "policy_digest": "digest_staged_cancel",
                "pending": {
                    "policy_digest": "digest_staged_cancel",
                    "expires_at_unix": expires,
                    "approval_present": false
                }
            }),
        ],
    );
    let _guard = SignerSocketGuard::set(&stub.sock_path);
    let mut view = SettingsView::new(0);

    open_edit_and_stage_42(&mut view, &mut wallet, &handle, &events);
    view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);

    let text = normalize_ws(&render(&view, &wallet));
    assert!(text.contains("max_position_pct"), "{text}");
    assert!(text.contains("not applied"), "{text}");
    assert!(
        text.contains("stays pending until it expires (10 min)"),
        "{text}"
    );

    let reqs = stub.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2, "Esc must never call apply: {reqs:?}");
}

#[test]
fn sentient_policy_expired_returns_to_editor_with_draft_and_applies_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = sentient_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let expires = future_unix_secs(600);
    let stub = spawn_signer_stub(
        &handle,
        vec![
            serde_json::json!({
                "ok": true,
                "policy": initial_policy_value(),
                "policy_digest": "digest_initial_0001"
            }),
            serde_json::json!({
                "ok": false,
                "code": "step_up_required",
                "message": "run: sudo vaughan signerd approve",
                "policy_digest": "digest_staged_exp",
                "pending": {
                    "policy_digest": "digest_staged_exp",
                    "expires_at_unix": expires,
                    "approval_present": false
                }
            }),
            serde_json::json!({ "ok": true, "pending": null }),
        ],
    );
    let _guard = SignerSocketGuard::set(&stub.sock_path);
    let mut view = SettingsView::new(0);

    open_edit_and_stage_42(&mut view, &mut wallet, &handle, &events);
    let flash = view.poll(1, &handle).unwrap();
    let expected = vaughan_core::core::policy_step_up::StepUpError::Expired.to_string();
    assert_eq!(flash, expected);

    let text = normalize_ws(&render_wide(&view, &wallet));
    assert!(text.contains("max_position_pct"), "{text}");
    assert!(text.contains("42"), "{text}");
    assert!(text.contains(&expected), "{text}");

    let reqs = stub.requests.lock().unwrap();
    assert_eq!(reqs.len(), 3);
    assert_eq!(reqs[2]["op"], "policy_pending");
}

#[test]
fn sentient_policy_poll_throttles_to_once_per_25_ticks() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = sentient_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let expires = future_unix_secs(600);
    let stub = spawn_signer_stub(
        &handle,
        vec![
            serde_json::json!({
                "ok": true,
                "policy": initial_policy_value(),
                "policy_digest": "digest_initial_0001"
            }),
            serde_json::json!({
                "ok": false,
                "code": "step_up_required",
                "message": "run: sudo vaughan signerd approve",
                "policy_digest": "digest_staged_thr",
                "pending": {
                    "policy_digest": "digest_staged_thr",
                    "expires_at_unix": expires,
                    "approval_present": false
                }
            }),
            serde_json::json!({
                "ok": true,
                "pending": {
                    "policy_digest": "digest_staged_thr",
                    "expires_at_unix": expires,
                    "approval_present": false
                }
            }),
            serde_json::json!({
                "ok": true,
                "pending": {
                    "policy_digest": "digest_staged_thr",
                    "expires_at_unix": expires,
                    "approval_present": false
                }
            }),
        ],
    );
    let _guard = SignerSocketGuard::set(&stub.sock_path);
    let mut view = SettingsView::new(0);

    open_edit_and_stage_42(&mut view, &mut wallet, &handle, &events);
    for tick in 1..=25 {
        assert!(view.poll(tick, &handle).is_none());
    }
    assert_eq!(stub.requests.lock().unwrap().len(), 3);

    assert!(view.poll(26, &handle).is_none());
    let reqs = stub.requests.lock().unwrap();
    assert_eq!(reqs.len(), 4);
    assert_eq!(reqs[2]["op"], "policy_pending");
    assert_eq!(reqs[3]["op"], "policy_pending");
}

#[test]
fn sentient_policy_happy_path_applies_edited_draft_not_daemon_payload() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = sentient_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let expires = future_unix_secs(600);
    let stub = spawn_signer_stub(
        &handle,
        vec![
            serde_json::json!({
                "ok": true,
                "policy": initial_policy_value(),
                "policy_digest": "digest_old_1111"
            }),
            serde_json::json!({
                "ok": false,
                "code": "step_up_required",
                "message": "run: sudo vaughan signerd approve",
                "policy_digest": "digest_staged_ok",
                "pending": {
                    "policy_digest": "digest_staged_ok",
                    "expires_at_unix": expires,
                    "approval_present": false
                }
            }),
            serde_json::json!({
                "ok": true,
                "pending": {
                    "policy_digest": "digest_staged_ok",
                    "expires_at_unix": expires,
                    "approval_present": true,
                    "policy": { "max_position_pct": 99 }
                }
            }),
            serde_json::json!({
                "ok": true,
                "policy_digest": "digest_new_3333"
            }),
        ],
    );
    let _guard = SignerSocketGuard::set(&stub.sock_path);
    let mut view = SettingsView::new(0);

    open_edit_and_stage_42(&mut view, &mut wallet, &handle, &events);
    let flash = view.poll(1, &handle).unwrap();
    assert_eq!(flash, "Sentient policy updated.");

    let text = normalize_ws(&render_wide(&view, &wallet));
    assert!(
        text.contains("Policy updated on signerd (digest digest_new_3333)."),
        "{text}"
    );
    assert!(
        !text.contains("max_position_pct"),
        "must return to Settings list after Applied:\n{text}"
    );

    let reqs = stub.requests.lock().unwrap();
    assert_eq!(reqs.len(), 4);
    assert_eq!(reqs[3]["op"], "policy_update");
    assert_eq!(reqs[3]["auth"], "approved");
    assert_eq!(reqs[3]["expected_digest"], "digest_old_1111");
    assert_eq!(reqs[3]["policy"]["max_position_pct"], 42);
}

#[test]
fn sentient_policy_conflict_on_apply_returns_to_editor_with_draft_intact() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = sentient_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let expires = future_unix_secs(600);
    let stub = spawn_signer_stub(
        &handle,
        vec![
            serde_json::json!({
                "ok": true,
                "policy": initial_policy_value(),
                "policy_digest": "digest_old_1111"
            }),
            serde_json::json!({
                "ok": false,
                "code": "step_up_required",
                "message": "run: sudo vaughan signerd approve",
                "policy_digest": "digest_staged_conf",
                "pending": {
                    "policy_digest": "digest_staged_conf",
                    "expires_at_unix": expires,
                    "approval_present": false
                }
            }),
            serde_json::json!({
                "ok": true,
                "pending": {
                    "policy_digest": "digest_staged_conf",
                    "expires_at_unix": expires,
                    "approval_present": true
                }
            }),
            serde_json::json!({
                "ok": false,
                "code": "policy_conflict",
                "message": "policy digest changed concurrently"
            }),
        ],
    );
    let _guard = SignerSocketGuard::set(&stub.sock_path);
    let mut view = SettingsView::new(0);

    open_edit_and_stage_42(&mut view, &mut wallet, &handle, &events);
    let flash = view.poll(1, &handle).unwrap();
    let expected = vaughan_core::core::policy_step_up::StepUpError::Daemon {
        code: "policy_conflict".into(),
        message: "policy digest changed concurrently".into(),
    }
    .to_string();
    assert_eq!(flash, expected);

    let text = normalize_ws(&render_wide(&view, &wallet));
    assert!(text.contains("max_position_pct"), "{text}");
    assert!(text.contains("42"), "{text}");
    assert!(text.contains(&expected), "{text}");
}

#[test]
fn sentient_policy_dead_daemon_on_poll_returns_error_without_hanging() {
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = sentient_wallet(dir.path());
    let (_rt, handle) = runtime_handle();
    let events = EventBus::new();
    let expires = future_unix_secs(600);
    let stub = spawn_signer_stub(
        &handle,
        vec![
            serde_json::json!({
                "ok": true,
                "policy": initial_policy_value(),
                "policy_digest": "digest_old_1111"
            }),
            serde_json::json!({
                "ok": false,
                "code": "step_up_required",
                "message": "run: sudo vaughan signerd approve",
                "policy_digest": "digest_staged_dead",
                "pending": {
                    "policy_digest": "digest_staged_dead",
                    "expires_at_unix": expires,
                    "approval_present": false
                }
            }),
        ],
    );
    let _guard = SignerSocketGuard::set(&stub.sock_path);
    let mut view = SettingsView::new(0);

    open_edit_and_stage_42(&mut view, &mut wallet, &handle, &events);
    drop(stub);

    let flash = view.poll(1, &handle).unwrap();
    let expected =
        vaughan_core::core::policy_step_up::StepUpError::Transport("signer_unavailable".into());
    assert!(
        flash.starts_with("signer transport error: signer_unavailable:")
            && flash.ends_with("; verify vaughan-signerd is running"),
        "expected StepUpError::Transport Display verbatim, got: {flash}"
    );
    let _ = expected;

    let text = normalize_ws(&render_wide(&view, &wallet));
    assert!(text.contains("max_position_pct"), "{text}");
    assert!(text.contains("42"), "{text}");
    assert!(text.contains(&flash[..flash.len().min(140)]), "{text}");
}
