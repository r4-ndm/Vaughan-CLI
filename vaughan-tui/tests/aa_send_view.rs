//! Tests for the TUI's AA batched-send view against a *forked* anvil.
//!
//! The fork carries PulseChain testnet state, where Ambire's real
//! `AmbireAccount` implementation is deployed, so the view exercises the
//! actual 7702 self-pay flow: compose transfers, confirm, broadcast, and the
//! batch executes on-chain against the real contract. Skips (does not fail)
//! when the testnet RPC is unreachable.

mod common;

use common::{anvil_dev_address, funded_wallet_at, render_frame, ForkedAnvil};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use vaughan_tui::views::AaSendView;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn char_key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn render(view: &AaSendView, wallet: &WalletState) -> String {
    render_frame(120, 30, |f| view.render(f, f.area(), wallet))
}

fn runtime_handle() -> (tokio::runtime::Runtime, Handle) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    (rt, handle)
}

/// Type `text` into the focused input of `view` using the real wallet/handle.
fn type_into(
    view: &mut AaSendView,
    text: &str,
    wallet: &WalletState,
    handle: &Handle,
    events: &EventBus,
) {
    for c in text.chars() {
        view.handle_key(char_key(c), wallet, handle, events);
    }
}

/// The full flow: compose a batch, confirm (fee + bootstrap note), broadcast,
/// and both transfers land on-chain through the real AmbireAccount contract.
#[test]
fn aa_send_view_broadcasts_batch_and_recipients_receive() {
    let Some(anvil) = ForkedAnvil::start() else {
        eprintln!("testnet RPC unreachable — skipping (network required for the fork)");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let wallet = funded_wallet_at(dir.path(), &anvil.url());
    let events = EventBus::new();

    let account = wallet.active_address().unwrap().to_string();
    let recipient1 = anvil_dev_address(1);
    let recipient2 = anvil_dev_address(2);

    let mut view = AaSendView::default();

    // Row 1: recipient + 1.5 tPLS (Enter advances recipient -> amount).
    type_into(&mut view, &recipient1[2..], &wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &wallet, &handle, &events);
    type_into(&mut view, "1.5", &wallet, &handle, &events);

    // Row 2: ctrl+a adds a row (cursor moves to it), fill recipient + 0.25.
    view.handle_key(ctrl('a'), &wallet, &handle, &events);
    type_into(&mut view, &recipient2[2..], &wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &wallet, &handle, &events);
    type_into(&mut view, "0.25", &wallet, &handle, &events);

    // Enter on the last row's amount -> confirm.
    view.handle_key(key(KeyCode::Enter), &wallet, &handle, &events);

    // Confirm screen: both rows, total, fee, and the one-time delegation note.
    let text = render(&view, &wallet);
    assert!(
        text.contains("Send batch of 2 transfers:"),
        "batch header:\n{text}"
    );
    // The view renders exactly what was typed (checksummed, no 0x prefix).
    assert!(text.contains(&recipient1[2..]), "row 1 recipient:\n{text}");
    assert!(text.contains(&recipient2[2..]), "row 2 recipient:\n{text}");
    assert!(text.contains("1.5 tPLS"), "row 1 amount:\n{text}");
    assert!(text.contains("0.25 tPLS"), "row 2 amount:\n{text}");
    assert!(text.contains("Fee:"), "fee must be shown:\n{text}");
    assert!(
        text.contains("First batch: EIP-7702 delegates this EOA to AmbireAccount"),
        "bootstrap note must be shown on a fresh account:\n{text}"
    );

    let before1 = anvil.wei_balance(&recipient1);
    let before2 = anvil.wei_balance(&recipient2);

    // Broadcast.
    view.handle_key(key(KeyCode::Enter), &wallet, &handle, &events);

    let text = render(&view, &wallet);
    assert!(
        text.contains("Transaction broadcast"),
        "done screen:\n{text}"
    );
    assert!(
        text.contains("Account delegated"),
        "bootstrap confirmation:\n{text}"
    );
    let hash_line = text
        .lines()
        .map(|l| l.trim().trim_matches('│').trim()) // strip the box border
        .find(|l| l.starts_with("0x") && l.len() >= 66)
        .map(|l| l.to_string())
        .unwrap_or_else(|| panic!("done screen must show a tx hash:\n{text}"));
    assert!(hash_line.len() >= 66, "tx hash looks wrong: {hash_line}");

    // Both transfers executed on-chain (bootstrap + one batch tx).
    assert_eq!(
        anvil.wei_balance(&recipient1),
        before1 + 15 * 10u128.pow(17),
        "row 1 transfer must land on-chain"
    );
    assert_eq!(
        anvil.wei_balance(&recipient2),
        before2 + 25 * 10u128.pow(16),
        "row 2 transfer must land on-chain"
    );
    // The account is now permanently delegated to the Ambire implementation.
    assert!(
        anvil.code(&account).starts_with("0xef01"),
        "account must be delegated after the batch"
    );
}

/// Esc on the confirm screen cancels: nothing is broadcast, the account is
/// not delegated, and the view returns to the editor.
#[test]
fn aa_send_view_esc_cancels_confirm() {
    let Some(anvil) = ForkedAnvil::start() else {
        eprintln!("testnet RPC unreachable — skipping (network required for the fork)");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let wallet = funded_wallet_at(dir.path(), &anvil.url());
    let events = EventBus::new();

    let account = wallet.active_address().unwrap().to_string();
    let recipient = anvil_dev_address(1);
    let before = anvil.wei_balance(&recipient);
    let mut view = AaSendView::default();

    type_into(&mut view, &recipient[2..], &wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &wallet, &handle, &events);
    type_into(&mut view, "0.5", &wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &wallet, &handle, &events);

    assert!(render(&view, &wallet).contains("Send batch of 1 transfers:"));

    view.handle_key(key(KeyCode::Esc), &wallet, &handle, &events);

    // Back in the editor, nothing broadcast, account untouched.
    let text = render(&view, &wallet);
    assert!(
        text.contains("ctrl+a add"),
        "must be back in the editor:\n{text}"
    );
    assert_eq!(
        anvil.wei_balance(&recipient),
        before,
        "nothing may land on-chain"
    );
    assert_eq!(anvil.code(&account), "0x", "account must not be delegated");
}

/// An invalid amount never leaves the editor; nothing is broadcast.
#[test]
fn aa_send_view_invalid_amount_stays_on_edit() {
    let Some(anvil) = ForkedAnvil::start() else {
        eprintln!("testnet RPC unreachable — skipping (network required for the fork)");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let wallet = funded_wallet_at(dir.path(), &anvil.url());
    let events = EventBus::new();

    let recipient = anvil_dev_address(1);
    let before = anvil.wei_balance(&recipient);
    let mut view = AaSendView::default();

    type_into(&mut view, &recipient[2..], &wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &wallet, &handle, &events);
    type_into(&mut view, "abc", &wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &wallet, &handle, &events);

    let text = render(&view, &wallet);
    assert!(
        text.contains("ctrl+a add"),
        "must stay in the editor:\n{text}"
    );
    assert!(
        text.to_lowercase().contains("invalid"),
        "must surface the parse error:\n{text}"
    );
    assert_eq!(
        anvil.wei_balance(&recipient),
        before,
        "nothing may land on-chain"
    );
}

/// An empty batch is rejected before any RPC work; the view stays on edit.
#[test]
fn aa_send_view_empty_batch_is_rejected() {
    let Some(anvil) = ForkedAnvil::start() else {
        eprintln!("testnet RPC unreachable — skipping (network required for the fork)");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let wallet = funded_wallet_at(dir.path(), &anvil.url());
    let events = EventBus::new();

    let mut view = AaSendView::default();
    // Enter moves recipient -> amount; a second Enter submits the (empty)
    // batch and must be rejected before any RPC work.
    view.handle_key(key(KeyCode::Enter), &wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &wallet, &handle, &events);

    let text = render(&view, &wallet);
    assert!(
        text.contains("ctrl+a add"),
        "must stay in the editor:\n{text}"
    );
    assert!(
        text.contains("at least one transfer"),
        "must explain the empty batch:\n{text}"
    );
    let _ = anvil;
}
