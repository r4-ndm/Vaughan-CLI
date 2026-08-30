//! Assets: balances, import custom ERC-20s, send native or token.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::chains::Balance;
use vaughan_core::core::{format_display_amount, token_launch_allowed, WalletState};
use vaughan_core::error::WalletError;
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, UiJob};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    List,
    Import,
}

pub struct AssetsView {
    assets: Vec<Balance>,
    selected: usize,
    loading: bool,
    tick: u64,
    stage: Stage,
    import_addr: Input,
    status: String,
}

impl Default for AssetsView {
    fn default() -> Self {
        Self::loading()
    }
}

impl AssetsView {
    pub fn loading() -> Self {
        Self {
            assets: Vec::new(),
            selected: 0,
            loading: true,
            tick: 0,
            stage: Stage::List,
            import_addr: Input::new(false, "0x… token contract"),
            status: String::new(),
        }
    }

    pub fn with_assets(result: Result<Vec<Balance>, WalletError>) -> Self {
        let mut v = Self::loading();
        v.apply_assets(result);
        v
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn apply_assets(&mut self, result: Result<Vec<Balance>, WalletError>) {
        self.loading = false;
        match result {
            Ok(assets) => {
                self.assets = assets;
                if self.selected >= self.assets.len() {
                    self.selected = self.assets.len().saturating_sub(1);
                }
                self.status.clear();
            }
            Err(e) => self.status = e.user_message(),
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let net = wallet.networks().active();
        let testnet = if net.is_testnet { " (testnet)" } else { "" };

        match self.stage {
            Stage::Import => {
                let [msg, field] =
                    Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(content);
                let msg_inner =
                    brand::render_faded_box(frame, msg, Some(brand::fade_line(" Import token ")));
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from("Import a custom ERC-20 (meme coin, etc.)"),
                        Line::from("Paste the contract address, Enter to import, Esc cancel."),
                    ])
                    .wrap(Wrap { trim: false }),
                    msg_inner,
                );
                render_labeled_input(frame, field, "Token", &self.import_addr, true);
            }
            Stage::List => {
                let items: Vec<ListItem> = if self.loading {
                    vec![ListItem::new(Line::from(format!(
                        "  {} loading assets…",
                        spinner_frame(self.tick)
                    )))]
                } else if self.assets.is_empty() {
                    vec![ListItem::new(Line::from(
                        "  No balances — press i to import a token.",
                    ))]
                } else {
                    self.assets
                        .iter()
                        .enumerate()
                        .map(|(i, b)| {
                            let mark = if i == self.selected { ">" } else { " " };
                            let kind = if b.token.contract_address.is_none() {
                                "native"
                            } else {
                                "ERC-20"
                            };
                            let label = format!(
                                "{mark} {:<12} {:>22}  ({kind})",
                                b.token.symbol,
                                format_display_amount(&b.raw, b.token.decimals, 6)
                            );
                            let style = if i == self.selected {
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

                let title = format!(
                    " Assets — {}{testnet} (↑↓ · Enter send · i import · u launch · r refresh · Esc) ",
                    net.name
                );
                let inner = brand::render_faded_box(frame, content, Some(brand::fade_line(&title)));
                frame.render_widget(List::new(items), inner);
            }
        }
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        matches!(self.stage, Stage::List)
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        match self.stage {
            Stage::Import => {
                if key.code == KeyCode::Esc {
                    self.stage = Stage::List;
                    self.status.clear();
                    return KeyOutcome::Consumed;
                }
                match self.import_addr.handle_key(key) {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    InputAction::Consumed => KeyOutcome::Consumed,
                    InputAction::Submitted => {
                        let addr = self.import_addr.value().to_string();
                        match handle.block_on(wallet.import_custom_token(&addr)) {
                            Ok(token) => {
                                self.import_addr.set_value("");
                                self.stage = Stage::List;
                                self.status =
                                    format!("Imported {} ({})", token.symbol, token.address);
                                KeyOutcome::StartJob(UiJob::RefreshAssets)
                            }
                            Err(e) => {
                                self.status = e.user_message();
                                KeyOutcome::Consumed
                            }
                        }
                    }
                }
            }
            Stage::List => match key.code {
                KeyCode::Esc => KeyOutcome::Back,
                KeyCode::Char('r') => KeyOutcome::StartJob(UiJob::RefreshAssets),
                KeyCode::Char('i') => {
                    self.stage = Stage::Import;
                    self.status.clear();
                    KeyOutcome::Consumed
                }
                KeyCode::Char('u') if token_launch_allowed(wallet.networks().active().chain_id) => {
                    KeyOutcome::Navigate(Screen::TokenLaunch)
                }
                KeyCode::Up => {
                    self.selected = self.selected.saturating_sub(1);
                    KeyOutcome::Consumed
                }
                KeyCode::Down => {
                    if !self.assets.is_empty() {
                        self.selected = (self.selected + 1).min(self.assets.len() - 1);
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    if let Some(bal) = self.assets.get(self.selected).cloned() {
                        KeyOutcome::SendAsset(bal)
                    } else {
                        KeyOutcome::Consumed
                    }
                }
                KeyCode::Char('x') => {
                    if let Some(addr) = self
                        .assets
                        .get(self.selected)
                        .and_then(|b| b.token.contract_address.clone())
                    {
                        match wallet.remove_custom_token(&addr) {
                            Ok(()) => {
                                self.status = "Removed custom token.".into();
                                KeyOutcome::StartJob(UiJob::RefreshAssets)
                            }
                            Err(e) => {
                                self.status = e.user_message();
                                KeyOutcome::Consumed
                            }
                        }
                    } else {
                        self.status = "Only custom imports can be removed (x).".into();
                        KeyOutcome::Consumed
                    }
                }
                _ => KeyOutcome::NotHandled,
            },
        }
    }
}
