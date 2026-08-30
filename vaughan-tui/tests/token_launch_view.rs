//! Headless render tests for the token launch view (deploy progress bar).

mod common;

use common::{funded_wallet, render_frame, Anvil};
use vaughan_core::core::WalletState;
use vaughan_tui::views::TokenLaunchView;

fn render(view: &TokenLaunchView, wallet: &WalletState) -> String {
    render_frame(100, 28, |f| view.render(f, f.area(), wallet))
}

#[test]
fn deploying_shows_progress_bar_and_summary() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let mut view = TokenLaunchView::for_chain(943);
    view.begin_deploying("Test Meme".into(), "MEME".into(), "1000000".into());
    view.set_tick(12);
    let text = render(&view, &wallet);
    assert!(text.contains("Deploying token"), "title missing:\n{text}");
    assert!(
        text.contains("MEME"),
        "ticker missing from summary:\n{text}"
    );
    assert!(
        text.contains("deploy in progress") || text.contains("Deploying MEME"),
        "status/progress hint missing:\n{text}"
    );
}
