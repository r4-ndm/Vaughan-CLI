//! Settings / Net: switch networks; add or remove custom EVM networks.
//!
//! Footer `n` / `i` both land here. Built-ins are fixed; customs persist in the vault.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::{EventBus, ProviderEvent};

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    List,
    Add,
    /// Ledger USB / Linux udev help (no secrets).
    HardwareHelp,
}

pub struct SettingsView {
    stage: Stage,
    selected: usize,
    /// Add-form fields: name, chain_id, rpc_url, symbol.
    focus: usize,
    name: Input,
    chain_id: Input,
    rpc_url: Input,
    symbol: Input,
    /// New customs default to testnet (safer); Space toggles in Add.
    is_testnet: bool,
    status: String,
}

impl SettingsView {
    pub fn new(selected: usize) -> Self {
        Self {
            stage: Stage::List,
            selected,
            focus: 0,
            name: Input::new(false, "Anvil / My Chain"),
            chain_id: Input::new(false, "31337"),
            rpc_url: Input::new(false, "http://127.0.0.1:8545"),
            symbol: Input::new(false, "ETH"),
            is_testnet: true,
            status: String::new(),
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);

        match self.stage {
            Stage::List => {
                let networks = wallet.networks();
                let active_id = networks.active_id();
                let items: Vec<ListItem> = networks
                    .networks()
                    .iter()
                    .enumerate()
                    .map(|(i, net)| {
                        let mark = if net.id == active_id { " * " } else { "   " };
                        let kind = if networks.is_custom(&net.id) {
                            " [custom]"
                        } else {
                            ""
                        };
                        let test = if net.is_testnet { " testnet" } else { "" };
                        let label = format!(
                            "{mark}{}{kind}  ({})  chain {}{test}",
                            net.name, net.native_symbol, net.chain_id
                        );
                        let style = if i == self.selected {
                            Style::default().fg(Color::Black).bg(Color::Cyan)
                        } else {
                            Style::default()
                        };
                        ListItem::new(Line::from(Span::styled(label, style)))
                    })
                    .collect();

                let list = List::new(items);
                let cdp_label = if wallet.agent_browser_control() {
                    "Agent browser control (CDP): ON"
                } else {
                    "Agent browser control (CDP): OFF"
                };
                let cdp_style = if wallet.agent_browser_control() {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let [list_area, hint_area, cdp_area] = Layout::vertical([
                    Constraint::Min(4),
                    Constraint::Length(3),
                    Constraint::Length(1),
                ])
                .areas(content);
                let inner = brand::render_faded_box(
                    frame,
                    list_area,
                    Some(brand::fade_line(
                        " Networks (↑↓ Enter · a add · d delete · h HW USB · Esc) ",
                    )),
                );
                frame.render_widget(list, inner);
                let hint_inner = brand::render_faded_box(frame, hint_area, None);
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "h — Ledger/Trezor USB help (Linux udev) · k — Keys · p — agent browser CDP",
                        Style::default().fg(Color::DarkGray),
                    ))),
                    hint_inner,
                );
                let cdp_inner = brand::render_faded_box(frame, cdp_area, None);
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(cdp_label, cdp_style))),
                    cdp_inner,
                );
            }
            Stage::HardwareHelp => {
                let inner = brand::render_faded_box(
                    frame,
                    content,
                    Some(brand::fade_line(" Hardware USB (Linux) ")),
                );
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(Span::styled(
                            "Official vendor docs only (no third-party scripts)",
                            Style::default().fg(Color::Yellow),
                        )),
                        Line::from(""),
                        Line::from(Span::styled("Ledger", Style::default().fg(Color::Cyan))),
                        Line::from("  support.ledger.com/article/115005165269-zd"),
                        Line::from("  Then: unlock · Ethereum app · Keys → 4 Add Ledger"),
                        Line::from(""),
                        Line::from(Span::styled("Trezor", Style::default().fg(Color::Cyan))),
                        Line::from("  trezor.io/guides/trezorctl/udev-rules"),
                        Line::from("  (Vaughan Trezor signing is Phase 2)"),
                        Line::from(""),
                        Line::from("After either guide: re-login if asked, replug the device."),
                        Line::from(""),
                        Line::from("Esc — back to networks"),
                    ])
                    .wrap(Wrap { trim: false }),
                    inner,
                );
            }
            Stage::Add => {
                let [msg, name_a, chain_a, rpc_a, sym_a] = Layout::vertical([
                    Constraint::Min(2),
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(3),
                ])
                .areas(content);
                let test_label = if self.is_testnet {
                    "testnet = yes (Space toggle)"
                } else {
                    "testnet = no  (Space toggle) — mainnet funds at risk"
                };
                let msg_inner = brand::render_faded_box(
                    frame,
                    msg,
                    Some(brand::fade_line(" Add custom network ")),
                );
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(
                            "Name · chain id · RPC · symbol · Tab fields · Enter save · Esc",
                        ),
                        Line::from(test_label),
                    ])
                    .wrap(Wrap { trim: false }),
                    msg_inner,
                );
                render_labeled_input(frame, name_a, "Name", &self.name, self.focus == 0);
                render_labeled_input(frame, chain_a, "Chain id", &self.chain_id, self.focus == 1);
                render_labeled_input(frame, rpc_a, "RPC URL", &self.rpc_url, self.focus == 2);
                render_labeled_input(frame, sym_a, "Symbol", &self.symbol, self.focus == 3);
            }
        }

        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        _handle: &Handle,
        events: &EventBus,
    ) -> KeyOutcome {
        match self.stage {
            Stage::List => self.handle_list_key(key, wallet, events),
            Stage::Add => self.handle_add_key(key, wallet, events),
            Stage::HardwareHelp => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::Consumed,
            },
        }
    }

    fn handle_list_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        events: &EventBus,
    ) -> KeyOutcome {
        let len = wallet.networks().networks().len();
        match key.code {
            KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
            KeyCode::Char('k') => KeyOutcome::Navigate(Screen::Keys),
            KeyCode::Char('w') => KeyOutcome::Navigate(Screen::Dapps),
            KeyCode::Char('h') => {
                self.stage = Stage::HardwareHelp;
                self.status.clear();
                KeyOutcome::Consumed
            }
            KeyCode::Char('p') => {
                let next = !wallet.agent_browser_control();
                match wallet.set_agent_browser_control(next) {
                    Ok(()) => {
                        self.status = if next {
                            "Agent browser control (CDP) enabled — MCP browser_* tools may use loopback CDP."
                                .into()
                        } else {
                            "Agent browser control (CDP) disabled — close VB if open.".into()
                        };
                    }
                    Err(e) => self.status = e.user_message(),
                }
                KeyOutcome::Consumed
            }
            KeyCode::Char('a') => {
                self.stage = Stage::Add;
                self.focus = 0;
                self.is_testnet = true;
                self.status.clear();
                KeyOutcome::Consumed
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(len.saturating_sub(1));
                KeyOutcome::Consumed
            }
            KeyCode::Enter => {
                if let Some(net) = wallet.networks().networks().get(self.selected) {
                    let id = net.id.clone();
                    let name = net.name.clone();
                    let chain_id = net.chain_id;
                    match wallet.set_active_network(&id) {
                        Ok(()) => {
                            self.status = format!("Switched to {name}.");
                            events.publish(ProviderEvent::ChainChanged(format!("0x{chain_id:x}")));
                        }
                        Err(e) => self.status = e.user_message(),
                    }
                }
                KeyOutcome::Consumed
            }
            KeyCode::Char('d') => {
                let Some(net) = wallet.networks().networks().get(self.selected).cloned() else {
                    return KeyOutcome::Consumed;
                };
                if !wallet.networks().is_custom(&net.id) {
                    self.status = "Built-in networks cannot be removed.".into();
                    return KeyOutcome::Consumed;
                }
                let chain_id = net.chain_id;
                match wallet.remove_custom_network(&net.id) {
                    Ok(()) => {
                        self.status = format!("Removed {}.", net.name);
                        self.selected = self.selected.saturating_sub(1);
                        let active = wallet.networks().active();
                        events.publish(ProviderEvent::ChainChanged(format!(
                            "0x{:x}",
                            active.chain_id
                        )));
                        let _ = chain_id;
                    }
                    Err(e) => self.status = e.user_message(),
                }
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::NotHandled,
        }
    }

    fn handle_add_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        events: &EventBus,
    ) -> KeyOutcome {
        if key.code == KeyCode::Esc {
            self.stage = Stage::List;
            return KeyOutcome::Consumed;
        }
        if key.code == KeyCode::Char(' ') {
            self.is_testnet = !self.is_testnet;
            return KeyOutcome::Consumed;
        }
        if key.code == KeyCode::Tab {
            self.focus = (self.focus + 1) % 4;
            return KeyOutcome::Consumed;
        }

        let action = match self.focus {
            0 => self.name.handle_key(key),
            1 => self.chain_id.handle_key(key),
            2 => self.rpc_url.handle_key(key),
            _ => self.symbol.handle_key(key),
        };
        match action {
            InputAction::Ignored => KeyOutcome::NotHandled,
            InputAction::Consumed => KeyOutcome::Consumed,
            InputAction::Submitted if self.focus < 3 => {
                self.focus += 1;
                KeyOutcome::Consumed
            }
            InputAction::Submitted => self.submit_custom(wallet, events),
        }
    }

    fn submit_custom(&mut self, wallet: &mut WalletState, events: &EventBus) -> KeyOutcome {
        let chain_id: u64 = match self.chain_id.value().trim().parse() {
            Ok(id) => id,
            Err(_) => {
                self.status = "Chain id must be a positive number.".into();
                return KeyOutcome::Consumed;
            }
        };
        match wallet.add_custom_network(
            self.name.value(),
            chain_id,
            self.rpc_url.value(),
            self.symbol.value(),
            self.is_testnet,
        ) {
            Ok(net) => {
                self.status = format!("Added {} (chain {}).", net.name, net.chain_id);
                events.publish(ProviderEvent::ChainChanged(format!("0x{:x}", net.chain_id)));
                // Select the new network in the list.
                if let Some(i) = wallet
                    .networks()
                    .networks()
                    .iter()
                    .position(|n| n.id == net.id)
                {
                    self.selected = i;
                }
                self.name.set_value("");
                self.chain_id.set_value("");
                self.rpc_url.set_value("");
                self.symbol.set_value("");
                self.stage = Stage::List;
            }
            Err(e) => self.status = e.user_message(),
        }
        KeyOutcome::Consumed
    }
}
