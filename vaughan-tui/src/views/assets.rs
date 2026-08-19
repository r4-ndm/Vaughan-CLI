//! Assets: native + curated ERC-20 balances (auto asset detection).
//!
//! Reads `WalletState::assets()` (one Multicall3 batch on chains that have
//! it, sequential `balanceOf` otherwise — see `docs/optimizations.md`),
//! renders each non-zero balance with on-chain symbol/decimals, and lets the
//! user refresh (`r`) or return to the dashboard (`Esc`/`d`).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::chains::Balance;
use vaughan_core::core::WalletState;
use vaughan_core::error::WalletError;
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::views::{body_areas, status_paragraph};

#[derive(Default)]
pub struct AssetsView {
    assets: Vec<Balance>,
    status: String,
}

impl AssetsView {
    pub fn with_assets(result: Result<Vec<Balance>, WalletError>) -> Self {
        match result {
            Ok(assets) => Self {
                assets,
                status: String::new(),
            },
            Err(e) => Self {
                assets: Vec::new(),
                status: e.user_message(),
            },
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let net = wallet.networks().active();
        let testnet = if net.is_testnet { " (testnet)" } else { "" };

        let items: Vec<ListItem> = if self.assets.is_empty() {
            vec![ListItem::new(Line::from("  No non-zero balances found."))]
        } else {
            self.assets
                .iter()
                .map(|b| {
                    let label = format!("  {:<20} {}", b.token.symbol, b.formatted);
                    let style = if b.token.contract_address.is_none() {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(Span::styled(label, style)))
                })
                .collect()
        };

        let list = List::new(items).block(Block::default().borders(Borders::ALL).title(format!(
            " Assets — {}{testnet} (r refresh, d back) ",
            net.name
        )));
        frame.render_widget(list, content);
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        match key.code {
            KeyCode::Esc | KeyCode::Char('d') => KeyOutcome::Navigate(Screen::Dashboard),
            KeyCode::Char('r') => {
                self.refresh(wallet, handle);
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::NotHandled,
        }
    }

    fn refresh(&mut self, wallet: &WalletState, handle: &Handle) {
        match handle.block_on(wallet.assets()) {
            Ok(assets) => {
                self.assets = assets;
                self.status.clear();
            }
            Err(e) => self.status = e.user_message(),
        }
    }
}
