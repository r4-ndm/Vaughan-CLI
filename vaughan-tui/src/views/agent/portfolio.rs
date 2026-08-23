//! In-session portfolio overlay (`p` / `/portfolio`).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, List, ListItem},
    Frame,
};
use vaughan_core::chains::Balance;
use vaughan_core::error::WalletError;

use super::AgentView;
use crate::app::KeyOutcome;
use crate::brand;
use crate::jobs::{spinner_frame, UiJob};

/// In-session portfolio panel (native + imported ERC-20 balances).
pub(super) struct PortfolioOverlay {
    pub(super) assets: Vec<Balance>,
    pub(super) selected: usize,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
}

impl PortfolioOverlay {
    pub(super) fn loading() -> Self {
        Self {
            assets: Vec::new(),
            selected: 0,
            loading: true,
            error: None,
        }
    }
}

impl AgentView {
    /// Apply a background [`UiJob::RefreshAssets`] result to the open portfolio.
    pub fn apply_portfolio(&mut self, result: Result<Vec<Balance>, WalletError>) {
        let Some(panel) = self.portfolio.as_mut() else {
            return;
        };
        panel.loading = false;
        match result {
            Ok(assets) => {
                panel.assets = assets;
                if panel.selected >= panel.assets.len() {
                    panel.selected = panel.assets.len().saturating_sub(1);
                }
                panel.error = None;
            }
            Err(e) => {
                panel.error = Some(e.user_message());
            }
        }
    }

    pub(super) fn open_portfolio(&mut self) -> KeyOutcome {
        self.portfolio = Some(PortfolioOverlay::loading());
        self.status.clear();
        KeyOutcome::StartJob(UiJob::RefreshAssets)
    }

    pub(super) fn handle_portfolio_key(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                self.portfolio = None;
                self.status.clear();
                KeyOutcome::Consumed
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if let Some(panel) = self.portfolio.as_mut() {
                    panel.loading = true;
                    panel.error = None;
                }
                KeyOutcome::StartJob(UiJob::RefreshAssets)
            }
            KeyCode::Up => {
                if let Some(panel) = self.portfolio.as_mut() {
                    if panel.selected > 0 {
                        panel.selected -= 1;
                    }
                }
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                if let Some(panel) = self.portfolio.as_mut() {
                    if !panel.assets.is_empty() && panel.selected + 1 < panel.assets.len() {
                        panel.selected += 1;
                    }
                }
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::Consumed,
        }
    }

    pub(super) fn render_portfolio(&self, frame: &mut Frame, area: Rect, panel: &PortfolioOverlay) {
        let net = &self.session.network_name;
        let testnet = if self.session.is_testnet {
            " (testnet)"
        } else {
            ""
        };
        let title = format!(" Portfolio — {net}{testnet} · Esc close · r refresh ");

        let items: Vec<ListItem> = if panel.loading {
            vec![ListItem::new(Line::from(format!(
                "  {} loading balances…",
                spinner_frame(self.tick)
            )))]
        } else if let Some(err) = &panel.error {
            vec![ListItem::new(Line::from(Span::styled(
                format!("  {err}"),
                Style::default().fg(Color::Red),
            )))]
        } else if panel.assets.is_empty() {
            vec![ListItem::new(Line::from(
                "  No balances — import tokens from Assets (dashboard), then r to refresh.",
            ))]
        } else {
            panel
                .assets
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let mark = if i == panel.selected { ">" } else { " " };
                    let kind = if b.token.contract_address.is_none() {
                        "native"
                    } else {
                        "ERC-20"
                    };
                    let label = format!(
                        "{mark} {:<12} {:>22}  ({kind})",
                        b.token.symbol, b.formatted
                    );
                    let style = if i == panel.selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else if b.token.contract_address.is_none() {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(Span::styled(label, style)))
                })
                .collect()
        };

        frame.render_widget(Clear, area);
        let list = List::new(items);
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(&title)));
        frame.render_widget(list, inner);
    }
}
