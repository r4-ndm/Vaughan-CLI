//! End-to-end tests for the TUI's native send view against a local Anvil node.
//!
//! Drives the real `SendView` state machine (Input → Confirm → Done) with real
//! key events, renders it headlessly via ratatui's `TestBackend`, and verifies
//! the broadcast on-chain (balance, nonce, tx receipt).
//!
//! These run on a plain thread (like the real UI thread), so the view's
//! `handle.block_on` fee-estimation and broadcast calls are safe.
//!
//! Requires `anvil` and `cast` on PATH. Run with:
//! ```sh
//! cargo test -p vaughan-tui --test send_view -- --nocapture
//! ```

mod common;

use std::time::{Duration, Instant};

use common::{anvil_dev_address, funded_wallet, render_frame, Anvil};
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::json;
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use vaughan_tui::views::SendView;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

/// Type one character into the focused field.
fn type_char(
    view: &mut SendView,
    c: char,
    wallet: &WalletState,
    handle: &Handle,
    events: &EventBus,
) {
    view.handle_key(key(KeyCode::Char(c)), wallet, handle, events);
}

/// Type a whole string into the focused field.
fn type_text(
    view: &mut SendView,
    text: &str,
    wallet: &WalletState,
    handle: &Handle,
    events: &EventBus,
) {
    for c in text.chars() {
        type_char(view, c, wallet, handle, events);
    }
}

/// Render the view into a headless buffer and return its full text.
fn render(view: &SendView, wallet: &WalletState) -> String {
    render_frame(100, 30, |f| view.render(f, f.area(), wallet))
}

/// Extract the first `0x` + 64-hex transaction hash in `text`.
fn find_tx_hash(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    for (i, w) in bytes.windows(2).enumerate() {
        if w == b"0x" {
            let rest = &bytes[i + 2..];
            if rest.len() >= 64 && rest[..64].iter().all(u8::is_ascii_hexdigit) {
                return Some(text[i..i + 2 + 64].to_string());
            }
        }
    }
    None
}

/// Drive recipient + amount + Enter (estimate) + Enter (broadcast) and return
/// the final rendered text.
fn drive_send(
    view: &mut SendView,
    recipient: &str,
    amount: &str,
    wallet: &WalletState,
    handle: &Handle,
    events: &EventBus,
) {
    type_text(view, recipient, wallet, handle, events);
    view.handle_key(key(KeyCode::Tab), wallet, handle, events);
    type_text(view, amount, wallet, handle, events);
    view.handle_key(key(KeyCode::Enter), wallet, handle, events); // amount submitted → fee estimate
    view.handle_key(key(KeyCode::Enter), wallet, handle, events); // confirm → broadcast
}

/// Happy path: recipient + amount → confirm screen → broadcast → receipt.
/// The rendered UI shows the confirm prompt, then "Transaction broadcast"
/// with a tx hash that anvil's receipt confirms.
#[test]
fn send_view_broadcasts_and_shows_receipt() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let sender = wallet.active_address().unwrap().to_string();
    let recipient = anvil_dev_address(6);
    let before = anvil.wei_balance(&recipient);
    let nonce_before = anvil.nonce(&sender);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = SendView::default();

    let value_wei = 10u128.pow(18); // 1 tPLS

    // Fill the form: recipient, Tab, amount, Enter (→ confirm).
    type_text(&mut view, &recipient, &wallet, &handle, &events);
    view.handle_key(key(KeyCode::Tab), &wallet, &handle, &events);
    type_text(&mut view, "1", &wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &wallet, &handle, &events);

    // Confirm screen: fee shown, recipient shown, broadcast hint.
    let text = render(&view, &wallet);
    assert!(
        text.contains("broadcast"),
        "confirm stage must offer broadcast:\n{text}"
    );
    assert!(
        text.contains("Fee:"),
        "confirm stage must show the fee:\n{text}"
    );
    assert!(
        text.to_lowercase().contains(&recipient.to_lowercase()),
        "confirm stage must show the recipient:\n{text}"
    );

    // Nothing broadcast yet.
    assert_eq!(
        anvil.wei_balance(&recipient),
        before,
        "nothing moved before confirm"
    );

    // Enter → broadcast → done stage with the tx hash.
    view.handle_key(key(KeyCode::Enter), &wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(
        text.contains("Transaction broadcast"),
        "done stage must confirm the broadcast:\n{text}"
    );
    let tx_hash = find_tx_hash(&text).expect("done stage must show the tx hash");
    assert!(
        tx_hash.starts_with("0x") && tx_hash.len() == 66,
        "hash shape: {tx_hash}"
    );

    // The receipt exists, is mined (status 0x1), and matches the form input.
    let tx = anvil
        .rpc("eth_getTransactionByHash", json!([tx_hash]))
        .unwrap();
    assert_eq!(
        tx["to"].as_str().unwrap().to_lowercase(),
        recipient.to_lowercase(),
        "tx recipient must match the form"
    );
    assert_eq!(
        tx["value"].as_str().unwrap(),
        format!("{value_wei:#x}"),
        "tx value must match the form"
    );
    let receipt = anvil
        .rpc("eth_getTransactionReceipt", json!([tx_hash]))
        .unwrap();
    assert_eq!(
        receipt["status"].as_str(),
        Some("0x1"),
        "receipt must be mined"
    );

    // Funds moved and the sender nonce advanced.
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if anvil.wei_balance(&recipient) == before + value_wei {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(anvil.wei_balance(&recipient), before + value_wei);
    assert_eq!(
        anvil.nonce(&sender),
        nonce_before + 1,
        "sender nonce must advance"
    );
}

/// An amount far above the balance: the confirm → broadcast attempt fails
/// cleanly, nothing lands, and the view returns to the input stage.
#[test]
fn send_view_insufficient_funds_fails_cleanly() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let sender = wallet.active_address().unwrap().to_string();
    let recipient = anvil_dev_address(7);
    let before = anvil.wei_balance(&recipient);
    let nonce_before = anvil.nonce(&sender);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = SendView::default();

    // 1,000,000 tPLS — anvil dev accounts only hold 10,000.
    drive_send(&mut view, &recipient, "1000000", &wallet, &handle, &events);

    let text = render(&view, &wallet);
    assert!(
        !text.contains("Transaction broadcast"),
        "insufficient funds must not broadcast:\n{text}"
    );

    // Nothing moved, nonce untouched.
    assert_eq!(anvil.wei_balance(&recipient), before);
    assert_eq!(anvil.nonce(&sender), nonce_before);

    // The view is back at the input stage (form fields visible again).
    assert!(
        text.contains("Amount"),
        "view must return to the form:\n{text}"
    );
}

/// A non-numeric amount never leaves the input stage and never broadcasts.
#[test]
fn send_view_invalid_amount_stays_on_input() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let sender = wallet.active_address().unwrap().to_string();
    let recipient = anvil_dev_address(8);
    let before = anvil.wei_balance(&recipient);
    let nonce_before = anvil.nonce(&sender);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = SendView::default();

    drive_send(&mut view, &recipient, "abc", &wallet, &handle, &events);

    let text = render(&view, &wallet);
    assert!(
        !text.contains("broadcast"),
        "invalid amount must not reach confirm:\n{text}"
    );
    assert!(
        text.contains("Amount"),
        "view must stay on the form:\n{text}"
    );
    assert_eq!(anvil.wei_balance(&recipient), before);
    assert_eq!(anvil.nonce(&sender), nonce_before);
}

/// Esc on the confirm screen cancels the send and returns to the form
/// without broadcasting.
#[test]
fn send_view_esc_cancels_confirm() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let sender = wallet.active_address().unwrap().to_string();
    let recipient = anvil_dev_address(9);
    let before = anvil.wei_balance(&recipient);
    let nonce_before = anvil.nonce(&sender);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = SendView::default();

    // Reach the confirm stage.
    type_text(&mut view, &recipient, &wallet, &handle, &events);
    view.handle_key(key(KeyCode::Tab), &wallet, &handle, &events);
    type_text(&mut view, "0.5", &wallet, &handle, &events);
    view.handle_key(key(KeyCode::Enter), &wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(
        text.contains("broadcast"),
        "must reach confirm first:\n{text}"
    );

    // Esc cancels: back to the form, nothing broadcast.
    view.handle_key(key(KeyCode::Esc), &wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(
        !text.contains("broadcast"),
        "Esc must leave the confirm stage:\n{text}"
    );
    assert!(text.contains("Amount"), "back at the form:\n{text}");
    assert_eq!(anvil.wei_balance(&recipient), before);
    assert_eq!(anvil.nonce(&sender), nonce_before);
}
