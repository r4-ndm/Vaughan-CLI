//! End-to-end tests for the TUI's native send view against a local Anvil node.
//!
//! Drives the real `SendView` state machine (Input → Confirm → Done) with real
//! key events, renders it headlessly via ratatui's `TestBackend`, and verifies
//! the broadcast on-chain (balance, nonce, tx receipt).
//!
//! Jobs that the live app runs on a worker thread are applied inline here so
//! the tests stay single-threaded.
//!
//! Requires `anvil` and `cast` on PATH. Run with:
//! ```sh
//! cargo test -p vaughan-tui --test send_view -- --nocapture
//! ```

mod common;

use std::time::{Duration, Instant};

use common::{
    anvil_dev_address, funded_wallet, plant_announcer, render_frame, wallet_from_mnemonic, Anvil,
    BOB_MNEMONIC,
};
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::json;
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use vaughan_tui::app::KeyOutcome;
use vaughan_tui::jobs::{UiJob, UiJobResult};
use vaughan_tui::views::SendView;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

/// Apply a background job the same way the app's worker thread would.
fn run_job(view: &mut SendView, job: UiJob, wallet: &WalletState, handle: &Handle) {
    let result = match job {
        UiJob::RefreshBalance => UiJobResult::Balance(handle.block_on(wallet.balance())),
        UiJob::RefreshAssets => UiJobResult::Assets(handle.block_on(wallet.assets())),
        UiJob::EstimateFee { to, value_wei } => {
            UiJobResult::Fee(handle.block_on(wallet.estimate_fee(&to, &value_wei)))
        }
        UiJob::EstimateTokenFee { token, to, amount } => {
            UiJobResult::Fee(handle.block_on(wallet.estimate_token_fee(&token, &to, &amount)))
        }
        UiJob::SendWithFee { to, value_wei, fee } => {
            UiJobResult::Send(handle.block_on(wallet.send_with_fee(&to, &value_wei, &fee)))
        }
        UiJob::Send { to, value_wei } => {
            UiJobResult::Send(handle.block_on(wallet.send(&to, &value_wei)))
        }
        UiJob::SendToken { token, to, amount } => {
            UiJobResult::Send(handle.block_on(wallet.send_token(&token, &to, &amount)))
        }
        UiJob::SendTokenWithFee {
            token,
            to,
            amount,
            fee,
        } => UiJobResult::Send(
            handle.block_on(wallet.send_token_with_fee(&token, &to, &amount, &fee)),
        ),
        UiJob::SendStealth {
            announcement,
            value_wei,
        } => UiJobResult::SendStealth(
            handle.block_on(wallet.send_stealth(&announcement, &value_wei)),
        ),
        // Chrome / DEX / Ag / Bridge / unlock jobs are not used by SendView tests.
        UiJob::Unlock { .. }
        | UiJob::RefreshChrome
        | UiJob::SendEvm { .. }
        | UiJob::EstimateEvmFee { .. }
        | UiJob::SendEvmWithFee { .. }
        | UiJob::AggQuote { .. }
        | UiJob::AggCompareQuote { .. }
        | UiJob::DexQuote { .. }
        | UiJob::BridgeQuote { .. }
        | UiJob::RefreshActivity { .. }
        | UiJob::RefreshAllowances
        | UiJob::PollTxStatus { .. }
        | UiJob::RefreshBroadcastStatuses { .. }
        | UiJob::ReplaceBroadcast { .. }
        | UiJob::LpListPositions { .. }
        | UiJob::DeployToken { .. } => return,
    };
    view.apply_job_result(result);
}

fn press(
    view: &mut SendView,
    code: KeyCode,
    wallet: &WalletState,
    handle: &Handle,
    events: &EventBus,
) {
    if let KeyOutcome::StartJob(job) = view.handle_key(key(code), wallet, handle, events) {
        run_job(view, job, wallet, handle);
    }
}

/// Type one character into the focused field.
fn type_char(
    view: &mut SendView,
    c: char,
    wallet: &WalletState,
    handle: &Handle,
    events: &EventBus,
) {
    press(view, KeyCode::Char(c), wallet, handle, events);
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
    press(view, KeyCode::Tab, wallet, handle, events);
    type_text(view, amount, wallet, handle, events);
    press(view, KeyCode::Enter, wallet, handle, events); // amount submitted → fee estimate
    press(view, KeyCode::Enter, wallet, handle, events); // confirm → broadcast
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
    press(&mut view, KeyCode::Tab, &wallet, &handle, &events);
    type_text(&mut view, "1", &wallet, &handle, &events);
    press(&mut view, KeyCode::Enter, &wallet, &handle, &events);

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
    press(&mut view, KeyCode::Enter, &wallet, &handle, &events);
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
    press(&mut view, KeyCode::Tab, &wallet, &handle, &events);
    type_text(&mut view, "0.5", &wallet, &handle, &events);
    press(&mut view, KeyCode::Enter, &wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(
        text.contains("broadcast"),
        "must reach confirm first:\n{text}"
    );

    // Esc cancels: back to the form, nothing broadcast.
    press(&mut view, KeyCode::Esc, &wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(
        !text.contains("broadcast"),
        "Esc must leave the confirm stage:\n{text}"
    );
    assert!(text.contains("Amount"), "back at the form:\n{text}");
    assert_eq!(anvil.wei_balance(&recipient), before);
    assert_eq!(anvil.nonce(&sender), nonce_before);
}

/// Confirm screen lists Slow/Normal/Fast/Ape/Custom; digit keys and ↑↓ change fee.
#[test]
fn send_view_gas_speed_presets() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let recipient = anvil_dev_address(10);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = SendView::default();

    type_text(&mut view, &recipient, &wallet, &handle, &events);
    press(&mut view, KeyCode::Tab, &wallet, &handle, &events);
    type_text(&mut view, "0.01", &wallet, &handle, &events);
    press(&mut view, KeyCode::Enter, &wallet, &handle, &events);

    let text = render(&view, &wallet);
    for label in ["Slow", "Normal", "Fast", "Ape", "Custom"] {
        assert!(text.contains(label), "confirm must list {label}:\n{text}");
    }
    assert!(
        text.contains("[Normal]"),
        "default speed is Normal:\n{text}"
    );

    press(&mut view, KeyCode::Char('4'), &wallet, &handle, &events);
    let ape = render(&view, &wallet);
    assert!(ape.contains("[Ape]"), "4 selects Ape:\n{ape}");

    press(&mut view, KeyCode::Char('1'), &wallet, &handle, &events);
    let slow = render(&view, &wallet);
    assert!(slow.contains("[Slow]"), "1 selects Slow:\n{slow}");

    press(&mut view, KeyCode::Down, &wallet, &handle, &events);
    let normal = render(&view, &wallet);
    assert!(
        normal.contains("[Normal]"),
        "↓ from Slow → Normal:\n{normal}"
    );

    press(&mut view, KeyCode::Char('5'), &wallet, &handle, &events);
    let custom = render(&view, &wallet);
    assert!(custom.contains("[Custom]"), "5 selects Custom:\n{custom}");
    assert!(
        custom.contains("max fee (gwei)"),
        "Custom shows gwei field:\n{custom}"
    );
}

/// Selecting Ape on confirm broadcasts with the scaled maxFeePerGas (not a re-estimate).
#[test]
fn send_view_ape_preset_broadcasts_scaled_fee() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let recipient = anvil_dev_address(11);
    let before = anvil.wei_balance(&recipient);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = SendView::default();

    type_text(&mut view, &recipient, &wallet, &handle, &events);
    press(&mut view, KeyCode::Tab, &wallet, &handle, &events);
    type_text(&mut view, "0.01", &wallet, &handle, &events);
    press(&mut view, KeyCode::Enter, &wallet, &handle, &events);

    // Capture Normal vs Ape display totals from the confirm screen fee line,
    // then broadcast with Ape selected.
    let normal_text = render(&view, &wallet);
    assert!(normal_text.contains("[Normal]"), "{normal_text}");
    press(&mut view, KeyCode::Char('4'), &wallet, &handle, &events);
    let ape_text = render(&view, &wallet);
    assert!(ape_text.contains("[Ape]"), "{ape_text}");
    assert_ne!(
        fee_line(&normal_text),
        fee_line(&ape_text),
        "Ape fee display must differ from Normal"
    );

    let expected_max = handle.block_on(async {
        let base = wallet
            .estimate_fee(&recipient, &(10u128.pow(16)).to_string())
            .await
            .expect("estimate");
        let ape = base.with_speed(vaughan_core::chains::FeeSpeed::Ape);
        match ape.details {
            vaughan_core::chains::FeeDetails::Evm {
                max_fee_per_gas: Some(max),
                ..
            } => max.parse::<u128>().unwrap(),
            _ => panic!("ape max fee"),
        }
    });

    press(&mut view, KeyCode::Enter, &wallet, &handle, &events);
    let done = render(&view, &wallet);
    assert!(
        done.contains("Transaction broadcast"),
        "Ape confirm must broadcast:\n{done}"
    );
    let tx_hash = find_tx_hash(&done).expect("tx hash on done screen");

    let tx = anvil
        .rpc("eth_getTransactionByHash", json!([tx_hash]))
        .unwrap();
    let on_chain = u128::from_str_radix(
        tx["maxFeePerGas"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    assert_eq!(
        on_chain, expected_max,
        "on-chain maxFeePerGas must match Ape-scaled estimate"
    );
    assert_eq!(anvil.wei_balance(&recipient), before + 10u128.pow(16));
}

/// Custom gwei entry broadcasts with the typed maxFeePerGas.
#[test]
fn send_view_custom_gas_broadcasts() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let recipient = anvil_dev_address(12);
    let before = anvil.wei_balance(&recipient);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = SendView::default();

    type_text(&mut view, &recipient, &wallet, &handle, &events);
    press(&mut view, KeyCode::Tab, &wallet, &handle, &events);
    type_text(&mut view, "0.01", &wallet, &handle, &events);
    press(&mut view, KeyCode::Enter, &wallet, &handle, &events);

    press(&mut view, KeyCode::Char('5'), &wallet, &handle, &events);
    // Clear prefilled gwei, type an explicit 77 gwei.
    for _ in 0..24 {
        press(&mut view, KeyCode::Backspace, &wallet, &handle, &events);
    }
    type_text(&mut view, "77", &wallet, &handle, &events);
    let custom = render(&view, &wallet);
    assert!(custom.contains("[Custom]"), "{custom}");
    assert!(
        custom.contains("77.00 gwei") || custom.contains("max 77"),
        "{custom}"
    );

    press(&mut view, KeyCode::Enter, &wallet, &handle, &events);
    let done = render(&view, &wallet);
    assert!(
        done.contains("Transaction broadcast"),
        "Custom confirm must broadcast:\n{done}"
    );
    let tx_hash = find_tx_hash(&done).expect("tx hash");
    let tx = anvil
        .rpc("eth_getTransactionByHash", json!([tx_hash]))
        .unwrap();
    let on_chain = u128::from_str_radix(
        tx["maxFeePerGas"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    assert_eq!(on_chain, 77_000_000_000, "custom 77 gwei on-chain");
    assert_eq!(anvil.wei_balance(&recipient), before + 10u128.pow(16));
}

fn fee_line(screen: &str) -> String {
    screen
        .lines()
        .find(|line| line.contains("Fee:"))
        .unwrap_or("")
        .to_string()
}

/// `st:` recipient: confirm shows the one-time stealth address, then pay+announce
/// lands a funded note that `scan_stealth_notes` finds.
#[test]
fn send_view_stealth_uri_pay_and_announce() {
    let anvil = Anvil::start();
    plant_announcer(&anvil);
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let uri = wallet.stealth_uri().unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = SendView::default();

    type_text(&mut view, &uri, &wallet, &handle, &events);
    press(&mut view, KeyCode::Tab, &wallet, &handle, &events);
    type_text(&mut view, "1", &wallet, &handle, &events);
    press(&mut view, KeyCode::Enter, &wallet, &handle, &events);

    let text = render(&view, &wallet);
    assert!(
        text.contains("one-time stealth"),
        "confirm must flag a stealth payment:\n{text}"
    );

    press(&mut view, KeyCode::Enter, &wallet, &handle, &events);
    let text = render(&view, &wallet);
    assert!(
        text.contains("Stealth payment broadcast"),
        "done stage must mention stealth:\n{text}"
    );

    let notes = handle
        .block_on(wallet.scan_stealth_notes())
        .expect("scan after stealth send");
    assert_eq!(notes.len(), 1, "one funded stealth note after TUI send");
}

/// `st:` send without an announcer fails cleanly and does not broadcast.
#[test]
fn send_view_stealth_without_announcer() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let uri = wallet.stealth_uri().unwrap();
    let sender = wallet.active_address().unwrap().to_string();
    let nonce_before = anvil.nonce(&sender);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = SendView::default();
    drive_send(&mut view, &uri, "1", &wallet, &handle, &events);

    let text = render(&view, &wallet);
    assert!(
        !text.contains("Stealth payment broadcast"),
        "missing announcer must not reach done:\n{text}"
    );
    assert!(
        text.to_lowercase().contains("announcer") || text.contains("Amount"),
        "must report the missing announcer or return to the form:\n{text}"
    );
    assert_eq!(
        anvil.nonce(&sender),
        nonce_before,
        "nonce must be unchanged"
    );
}

/// A malformed `st:` URI never leaves the input stage.
#[test]
fn send_view_invalid_stealth_uri() {
    let anvil = Anvil::start();
    plant_announcer(&anvil);
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = SendView::default();
    drive_send(&mut view, "st:tpls:0x1234", "1", &wallet, &handle, &events);

    let text = render(&view, &wallet);
    assert!(
        !text.contains("one-time stealth") && !text.contains("broadcast"),
        "invalid st: URI must not reach confirm:\n{text}"
    );
    assert!(text.contains("Amount"), "must stay on the form:\n{text}");
}

/// Alice's send view pays Bob's stealth URI; Bob's scan finds the note.
#[test]
fn send_view_alice_pays_bob() {
    let anvil = Anvil::start();
    plant_announcer(&anvil);
    let alice_dir = tempfile::tempdir().unwrap();
    let bob_dir = tempfile::tempdir().unwrap();
    let alice = funded_wallet(alice_dir.path(), &anvil);
    let bob = wallet_from_mnemonic(bob_dir.path(), &anvil, BOB_MNEMONIC);
    let bob_uri = bob.stealth_uri().unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let events = EventBus::new();
    let mut view = SendView::default();
    drive_send(&mut view, &bob_uri, "1", &alice, &handle, &events);

    let text = render(&view, &alice);
    assert!(
        text.contains("Stealth payment broadcast"),
        "alice's send must complete:\n{text}"
    );

    let notes = handle.block_on(bob.scan_stealth_notes()).expect("bob scan");
    assert_eq!(notes.len(), 1, "bob must see alice's stealth payment");
}
