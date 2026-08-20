//! Tests for the TUI's assets view against a real anvil node.
//!
//! The assets view lists every detected balance — native + curated ERC-20s
//! (auto asset detection via Multicall3 batch, sequential fallback without).
//! Rendered headlessly via ratatui's `TestBackend`; balances are fetched
//! through the wallet against anvil (`r` refreshes), and `Esc`/`d` navigate
//! back to the dashboard.

mod common;

use common::{funded_wallet, render_frame, Anvil};
use crossterm::event::{KeyCode, KeyEvent};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;
use vaughan_tui::app::{KeyOutcome, Screen};
use vaughan_tui::views::AssetsView;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

fn render(view: &AssetsView, wallet: &WalletState) -> String {
    render_frame(100, 24, |f| view.render(f, f.area(), wallet))
}

fn runtime_handle() -> (tokio::runtime::Runtime, Handle) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    (rt, handle)
}

/// Assets are fetched from anvil (native balance, since plain anvil has no
/// curated ERC-20s or Multicall3) and rendered in the list.
#[test]
fn assets_view_renders_detected_balances() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let wallet = funded_wallet(dir.path(), &anvil);
    let assets = handle.block_on(wallet.assets()).unwrap();

    let view = AssetsView::with_assets(Ok(assets.clone()));
    let text = render(&view, &wallet);

    assert!(
        text.contains("Assets — PulseChain Testnet V4 (testnet)"),
        "must render the assets header with network:\\n{text}"
    );
    let native = assets
        .iter()
        .find(|b| b.token.contract_address.is_none())
        .expect("native balance present");
    assert!(
        text.contains(&native.token.symbol),
        "must render the native token symbol:\\n{text}"
    );
    assert!(
        text.contains(&native.formatted),
        "must render the native balance amount:\\n{text}"
    );
    assert!(
        text.contains("r refresh"),
        "must render the shortcut bar:\\n{text}"
    );
}

/// `r` re-fetches assets from the node; `Esc` and `d` return to the dashboard.
#[test]
fn assets_view_refresh_and_navigation() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let (_rt, handle) = runtime_handle();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let events = EventBus::new();
    let mut view = AssetsView::default();

    // Empty until refreshed (loading spinner / import hint).
    assert!(
        render(&view, &wallet).contains("import a token")
            || render(&view, &wallet).contains("loading"),
        "fresh assets view must show empty/loading state"
    );

    // `r` schedules an assets refresh job (app applies it on the UI thread).
    let outcome = view.handle_key(key(KeyCode::Char('r')), &mut wallet, &handle, &events);
    match outcome {
        KeyOutcome::StartJob(vaughan_tui::jobs::UiJob::RefreshAssets) => {
            view.apply_assets(handle.block_on(wallet.assets()));
        }
        other => panic!("expected RefreshAssets job, got {other:?}"),
    }
    let expected = handle.block_on(wallet.assets()).unwrap();
    let text = render(&view, &wallet);
    assert!(
        expected.iter().any(|b| text.contains(&b.token.symbol)),
        "refresh must populate the list:\\n{text}"
    );

    // `Esc` and `d` navigate back to the dashboard.
    assert!(matches!(
        view.handle_key(key(KeyCode::Esc), &mut wallet, &handle, &events),
        KeyOutcome::Navigate(Screen::Dashboard)
    ));
    assert!(matches!(
        view.handle_key(key(KeyCode::Char('d')), &mut wallet, &handle, &events),
        KeyOutcome::Navigate(Screen::Dashboard)
    ));
}

/// A failed assets fetch shows the error message instead of crashing.
///
/// (The view is constructed with the error directly — a live dead-RPC probe
/// would be rescued by the adapter's fallback RPCs, which is correct
/// behavior, so it wouldn't fail.)
#[test]
fn assets_view_error_sets_status() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);

    let view = AssetsView::with_assets(Err(vaughan_core::error::WalletError::RpcError(
        "dead rpc".to_string(),
    )));
    let text = render(&view, &wallet);
    assert!(
        text.contains("blockchain RPC returned an error"),
        "failed fetch must surface in the status area:\\n{text}"
    );
    // The list stays empty — nothing to render.
    assert!(
        text.contains("import a token") || text.contains("No balances"),
        "error view must not fabricate balances:\\n{text}"
    );
}
