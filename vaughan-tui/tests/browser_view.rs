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
    assert!(
        text.contains("write <fn>"),
        "must list write command:\n{text}"
    );
    assert!(text.contains("/swap"), "must list intent macros:\n{text}");
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
    assert!(matches!(outcome, KeyOutcome::Navigate(Screen::Dashboard)));

    // Esc with text in buffer clears text, doesn't navigate
    type_text(&mut view, "browse 0x123", &mut wallet, &handle, &events);
    let outcome2 = view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events);
    assert!(matches!(outcome2, KeyOutcome::Consumed));
}

#[test]
fn browser_view_intent_macros_navigate() {
    use vaughan_tui::intent::IntentNav;

    let tmp = tempdir().unwrap();
    let mut wallet = common::fresh_wallet(tmp.path());
    let mut view = BrowserView::default();
    let events = EventBus::new();
    let (_rt, handle) = runtime_handle();

    type_text(&mut view, "/swap 1", &mut wallet, &handle, &events);
    let o = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(
        matches!(
            o,
            KeyOutcome::Intent(IntentNav::Aggregator {
                amount: Some(ref a),
                token_out: None
            }) if a == "1"
        ),
        "got {o:?}"
    );

    type_text(&mut view, "/revoke", &mut wallet, &handle, &events);
    let o2 = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(matches!(o2, KeyOutcome::Intent(IntentNav::Approvals)));

    type_text(&mut view, "/stealth receive", &mut wallet, &handle, &events);
    let o3 = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    assert!(matches!(o3, KeyOutcome::Intent(IntentNav::Receive)));
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

/// Gated `writeraw` → fee confirm → broadcast (WPLS deposit selector).
#[test]
fn browser_view_writeraw_confirm_broadcasts_on_anvil() {
    use alloy::primitives::{address, Address, U256};
    use std::str::FromStr;
    use std::time::{Duration, Instant};
    use vaughan_tui::jobs::{UiJob, UiJobResult};
    use vaughan_tui::views::dex_calldata::{encode_balance_of_call, weth_deposit_selector};

    const MOCK_WPLS: Address = address!("0xA1077a294dDE1B09bB078844df40758a5D0f9a27");

    let anvil = common::Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = common::funded_wallet(dir.path(), &anvil);
    let events = EventBus::new();
    let (_rt, handle) = runtime_handle();
    let mut view = BrowserView::default();

    let runtime = include_str!("fixtures/mock_weth.runtime.hex")
        .trim()
        .trim_start_matches("0x");
    anvil
        .rpc(
            "anvil_setCode",
            json!([format!("{MOCK_WPLS:#x}"), format!("0x{runtime}")]),
        )
        .expect("anvil_setCode");

    type_text(
        &mut view,
        &format!("browse {MOCK_WPLS:#x}"),
        &mut wallet,
        &handle,
        &events,
    );
    view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);

    let sel = format!("0x{}", hex::encode(weth_deposit_selector()));
    let amount = "10000000000000000"; // 0.01 PLS
    type_text(
        &mut view,
        &format!("writeraw {sel} value {amount}"),
        &mut wallet,
        &handle,
        &events,
    );
    let outcome = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    let KeyOutcome::StartJob(UiJob::EstimateEvmFee { tx }) = outcome else {
        panic!("expected EstimateEvmFee, got {outcome:?}");
    };
    assert_eq!(tx.value, amount);
    assert_eq!(
        tx.to.to_lowercase(),
        format!("{MOCK_WPLS:#x}").to_lowercase()
    );

    let fee = handle
        .block_on(wallet.estimate_transaction_fee(tx.clone()))
        .expect("fee");
    view.apply_job_result(UiJobResult::Fee(Ok(fee.clone())));

    let text = common::render_frame(100, 30, |frame| view.render(frame, frame.area(), &wallet));
    assert!(
        text.contains("Confirm contract write") || text.contains("Est. fee"),
        "must show confirm card:\n{text}"
    );

    let outcome2 = view.handle_key(key(KeyCode::Enter), &mut wallet, &handle, &events);
    let KeyOutcome::StartJob(UiJob::SendEvmWithFee {
        tx: send_tx,
        fee: send_fee,
    }) = outcome2
    else {
        panic!("expected SendEvmWithFee, got {outcome2:?}");
    };
    assert_eq!(send_fee.total, fee.total);

    let receipt = handle
        .block_on(wallet.send_evm_with_fee(send_tx, &send_fee))
        .expect("broadcast");
    let hash = receipt.hash.clone();
    view.apply_job_result(UiJobResult::Send(Ok(receipt)));

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(r) = anvil.rpc("eth_getTransactionReceipt", json!([&hash])) {
            if !r.is_null() {
                assert_eq!(r["status"].as_str().unwrap_or("0x0"), "0x1");
                break;
            }
        }
        assert!(Instant::now() < deadline, "no receipt for {hash}");
        std::thread::sleep(Duration::from_millis(50));
    }

    let from = wallet.active_address().unwrap();
    let owner: Address = from.parse().unwrap();
    let bal_hex = anvil
        .rpc(
            "eth_call",
            json!([
                {
                    "to": format!("{MOCK_WPLS:#x}"),
                    "data": encode_balance_of_call(owner)
                },
                "latest"
            ]),
        )
        .expect("balanceOf");
    let bal = U256::from_str(bal_hex.as_str().unwrap()).unwrap_or_else(|_| {
        U256::from_str_radix(bal_hex.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
    });
    assert_eq!(bal, U256::from_str(amount).unwrap());
}
