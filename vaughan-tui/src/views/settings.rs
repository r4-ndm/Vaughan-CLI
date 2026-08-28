//! Settings / Net: switch networks; add, edit, or remove custom EVM networks.
//!
//! Footer `n` / `i` both land here. Built-ins are fixed; customs persist in the vault.
//! Built-in RPC: **`r`**. Custom chains: **`a`** add · **`e`** edit · **`d`** delete.

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
    /// Add or edit a custom network (see [`FormMode`]).
    Form,
    /// Ledger USB / Linux udev help (no secrets).
    HardwareHelp,
    /// Pick primary RPC for the highlighted network (built-in fallbacks stay active).
    RpcPick,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum FormMode {
    #[default]
    Add,
    Edit,
}

pub struct SettingsView {
    stage: Stage,
    selected: usize,
    form_mode: FormMode,
    /// Set when [`FormMode::Edit`] — custom network id (chain id is not editable).
    edit_network_id: String,
    edit_chain_id: u64,
    /// Add-form fields: name, chain_id, rpc_url, symbol.
    focus: usize,
    name: Input,
    chain_id: Input,
    rpc_url: Input,
    symbol: Input,
    /// New customs default to testnet (safer); Space toggles in Add.
    is_testnet: bool,
    /// Network id while in [`Stage::RpcPick`].
    rpc_network_id: String,
    rpc_pick_index: usize,
    rpc_custom: bool,
    rpc_custom_input: Input,
    status: String,
}

impl SettingsView {
    pub fn new(selected: usize) -> Self {
        Self {
            stage: Stage::List,
            selected,
            form_mode: FormMode::Add,
            edit_network_id: String::new(),
            edit_chain_id: 0,
            focus: 0,
            name: Input::new(false, "Anvil / My Chain"),
            chain_id: Input::new(false, "31337"),
            rpc_url: Input::new(false, "http://127.0.0.1:8545"),
            symbol: Input::new(false, "ETH"),
            is_testnet: true,
            rpc_network_id: String::new(),
            rpc_pick_index: 0,
            rpc_custom: false,
            rpc_custom_input: Input::new(false, "https://your-rpc.example"),
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
                let [list_area, footer_area] =
                    Layout::vertical([Constraint::Min(4), Constraint::Length(5)]).areas(content);
                let inner =
                    brand::render_faded_box(frame, list_area, Some(brand::fade_line(" Networks ")));
                frame.render_widget(list, inner);
                let footer_inner = brand::render_faded_box(frame, footer_area, None);
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(Span::styled(
                            "↑↓ Enter · a add · e edit · r RPC · d delete · h udev · k Keys · p agent CDP · Esc",
                            Style::default().fg(Color::DarkGray),
                        )),
                        Line::from(Span::styled(cdp_label, cdp_style)),
                    ]),
                    footer_inner,
                );
            }
            Stage::HardwareHelp => {
                let inner = brand::render_faded_box(
                    frame,
                    content,
                    Some(brand::fade_line(" Hardware udev rules (Linux) ")),
                );
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(Span::styled(
                            "Add udev rules — official vendor docs only (no third-party scripts)",
                            Style::default().fg(Color::Yellow),
                        )),
                        Line::from(
                            "Linux USB permissions so Vaughan can detect Ledger/Trezor over USB.",
                        ),
                        Line::from(""),
                        Line::from(Span::styled(
                            "Ledger — add udev rules",
                            Style::default().fg(Color::Cyan),
                        )),
                        Line::from("  support.ledger.com/article/115005165269-zd"),
                        Line::from("  Then: unlock · Ethereum app · Keys → 4 Add Ledger"),
                        Line::from(""),
                        Line::from(Span::styled(
                            "Trezor — add udev rules",
                            Style::default().fg(Color::Cyan),
                        )),
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
            Stage::RpcPick => {
                let net_name = wallet
                    .networks()
                    .get(&self.rpc_network_id)
                    .map(|n| n.name.clone())
                    .unwrap_or_else(|| self.rpc_network_id.clone());
                let endpoints = wallet.known_rpc_endpoints(&self.rpc_network_id);
                let (active_primary, _) = wallet
                    .networks()
                    .get(&self.rpc_network_id)
                    .map(|n| wallet.rpc_endpoints_for(n))
                    .unwrap_or_default();
                if self.rpc_custom {
                    let [msg, input_a] =
                        Layout::vertical([Constraint::Min(3), Constraint::Length(3)])
                            .areas(content);
                    let msg_inner = brand::render_faded_box(
                        frame,
                        msg,
                        Some(brand::fade_line(" Custom RPC URL ")),
                    );
                    frame.render_widget(
                        Paragraph::new(vec![
                            Line::from(format!("Network: {net_name}")),
                            Line::from("Enter https URL · Enter save · Esc cancel"),
                            Line::from(
                                "Other known nodes still used as fallbacks if this one fails.",
                            ),
                        ])
                        .wrap(Wrap { trim: false }),
                        msg_inner,
                    );
                    render_labeled_input(frame, input_a, "RPC URL", &self.rpc_custom_input, true);
                } else {
                    let [msg, list_area] =
                        Layout::vertical([Constraint::Length(4), Constraint::Min(4)])
                            .areas(content);
                    let msg_inner =
                        brand::render_faded_box(frame, msg, Some(brand::fade_line(" RPC URL ")));
                    frame.render_widget(
                        Paragraph::new(vec![
                            Line::from(format!("Network: {net_name}")),
                            Line::from("↑↓ · Enter · 0 default · c custom · Esc"),
                        ])
                        .wrap(Wrap { trim: false }),
                        msg_inner,
                    );
                    let items: Vec<ListItem> = endpoints
                        .iter()
                        .enumerate()
                        .map(|(i, ep)| {
                            let selected = ep.url == active_primary;
                            let mark = if selected { " * " } else { "   " };
                            let style = if i == self.rpc_pick_index {
                                Style::default().fg(Color::Black).bg(Color::Cyan)
                            } else if selected {
                                Style::default().fg(Color::Green)
                            } else {
                                Style::default()
                            };
                            ListItem::new(Line::from(Span::styled(
                                format!("{mark}{:<14} {}", ep.label, ep.url),
                                style,
                            )))
                        })
                        .collect();
                    let inner = brand::render_faded_box(frame, list_area, None);
                    frame.render_widget(List::new(items), inner);
                }
            }
            Stage::Form => {
                let title = match self.form_mode {
                    FormMode::Add => " Add custom network ",
                    FormMode::Edit => " Edit custom network ",
                };
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
                let chain_hint = match self.form_mode {
                    FormMode::Add => "Name · chain id · RPC · symbol · Tab · Enter save · Esc",
                    FormMode::Edit => {
                        "Name · RPC · symbol · Tab · Enter save · Esc (chain id fixed)"
                    }
                };
                let msg_inner = brand::render_faded_box(frame, msg, Some(brand::fade_line(title)));
                frame.render_widget(
                    Paragraph::new(vec![Line::from(chain_hint), Line::from(test_label)])
                        .wrap(Wrap { trim: false }),
                    msg_inner,
                );
                render_labeled_input(frame, name_a, "Name", &self.name, self.focus == 0);
                if self.form_mode == FormMode::Add {
                    render_labeled_input(
                        frame,
                        chain_a,
                        "Chain id",
                        &self.chain_id,
                        self.focus == 1,
                    );
                } else {
                    let chain_inner = brand::render_faded_box(frame, chain_a, None);
                    frame.render_widget(
                        Paragraph::new(Line::from(format!(
                            "Chain id: {} (fixed)",
                            self.edit_chain_id
                        ))),
                        chain_inner,
                    );
                }
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
            Stage::Form => self.handle_form_key(key, wallet, events),
            Stage::HardwareHelp => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::Consumed,
            },
            Stage::RpcPick => self.handle_rpc_pick_key(key, wallet),
        }
    }

    fn open_rpc_pick(&mut self, wallet: &WalletState) {
        let Some(net) = wallet.networks().networks().get(self.selected) else {
            return;
        };
        self.rpc_network_id = net.id.clone();
        let (primary, _) = wallet.rpc_endpoints_for(net);
        let endpoints = wallet.known_rpc_endpoints(&self.rpc_network_id);
        self.rpc_pick_index = endpoints.iter().position(|e| e.url == primary).unwrap_or(0);
        self.rpc_custom = false;
        self.rpc_custom_input.set_value("");
        self.stage = Stage::RpcPick;
        self.status.clear();
    }

    fn handle_rpc_pick_key(&mut self, key: KeyEvent, wallet: &mut WalletState) -> KeyOutcome {
        if self.rpc_custom {
            if key.code == KeyCode::Esc {
                self.rpc_custom = false;
                return KeyOutcome::Consumed;
            }
            match self.rpc_custom_input.handle_key(key) {
                InputAction::Ignored => KeyOutcome::NotHandled,
                InputAction::Consumed => KeyOutcome::Consumed,
                InputAction::Submitted => {
                    let url = self.rpc_custom_input.value().trim();
                    match wallet.set_network_rpc_primary(&self.rpc_network_id, Some(url)) {
                        Ok(()) => {
                            self.status =
                                format!("RPC set to {url} (fallbacks remain if this node fails).");
                            self.stage = Stage::List;
                        }
                        Err(e) => self.status = e.user_message(),
                    }
                    KeyOutcome::Consumed
                }
            }
        } else {
            let endpoints = wallet.known_rpc_endpoints(&self.rpc_network_id);
            match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    KeyOutcome::Consumed
                }
                KeyCode::Char('c') => {
                    self.rpc_custom = true;
                    KeyOutcome::Consumed
                }
                KeyCode::Char('0') => {
                    let is_custom = wallet
                        .networks()
                        .is_custom(&self.rpc_network_id.to_ascii_lowercase());
                    match wallet.set_network_rpc_primary(&self.rpc_network_id, None) {
                        Ok(()) => {
                            self.status = if is_custom {
                                "Custom networks have no built-in RPC default — edit the network (e) to change URL."
                                    .into()
                            } else {
                                "RPC reset to built-in default.".into()
                            };
                            self.stage = Stage::List;
                        }
                        Err(e) => self.status = e.user_message(),
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::Up => {
                    self.rpc_pick_index = self.rpc_pick_index.saturating_sub(1);
                    KeyOutcome::Consumed
                }
                KeyCode::Down => {
                    if !endpoints.is_empty() {
                        self.rpc_pick_index =
                            (self.rpc_pick_index + 1).min(endpoints.len().saturating_sub(1));
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    let Some(ep) = endpoints.get(self.rpc_pick_index) else {
                        return KeyOutcome::Consumed;
                    };
                    match wallet.set_network_rpc_primary(&self.rpc_network_id, Some(&ep.url)) {
                        Ok(()) => {
                            self.status = format!(
                                "RPC set to {} — other nodes used as fallbacks if down.",
                                ep.label
                            );
                            self.stage = Stage::List;
                        }
                        Err(e) => self.status = e.user_message(),
                    }
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::NotHandled,
            }
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
                self.form_mode = FormMode::Add;
                self.stage = Stage::Form;
                self.focus = 0;
                self.is_testnet = true;
                self.name.set_value("");
                self.chain_id.set_value("");
                self.rpc_url.set_value("");
                self.symbol.set_value("");
                self.status.clear();
                KeyOutcome::Consumed
            }
            KeyCode::Char('e') => {
                let Some(net) = wallet.networks().networks().get(self.selected) else {
                    return KeyOutcome::Consumed;
                };
                if wallet.networks().is_custom(&net.id) {
                    self.open_edit_form(wallet, net);
                } else {
                    self.open_rpc_pick(wallet);
                    self.status = "Built-in chain — edit RPC here (name/symbol are fixed).".into();
                }
                KeyOutcome::Consumed
            }
            KeyCode::Char('r') => {
                self.open_rpc_pick(wallet);
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

    fn open_edit_form(
        &mut self,
        wallet: &WalletState,
        net: &vaughan_core::chains::evm::networks::EvmNetworkConfig,
    ) {
        self.form_mode = FormMode::Edit;
        self.edit_network_id = net.id.clone();
        self.edit_chain_id = net.chain_id;
        self.name.set_value(&net.name);
        let (primary, _) = wallet.rpc_endpoints_for(net);
        self.rpc_url.set_value(&primary);
        self.symbol.set_value(&net.native_symbol);
        self.is_testnet = net.is_testnet;
        self.focus = 0;
        self.stage = Stage::Form;
        self.status.clear();
    }

    fn handle_form_key(
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
            self.focus = match self.form_mode {
                FormMode::Add => (self.focus + 1) % 4,
                FormMode::Edit => match self.focus {
                    0 => 2,
                    2 => 3,
                    _ => 0,
                },
            };
            return KeyOutcome::Consumed;
        }

        let action = match self.focus {
            0 => self.name.handle_key(key),
            1 if self.form_mode == FormMode::Add => self.chain_id.handle_key(key),
            2 => self.rpc_url.handle_key(key),
            _ => self.symbol.handle_key(key),
        };
        match action {
            InputAction::Ignored => KeyOutcome::NotHandled,
            InputAction::Consumed => KeyOutcome::Consumed,
            InputAction::Submitted => match self.form_mode {
                FormMode::Add if self.focus < 3 => {
                    self.focus += 1;
                    KeyOutcome::Consumed
                }
                FormMode::Edit if self.focus == 0 => {
                    self.focus = 2;
                    KeyOutcome::Consumed
                }
                FormMode::Edit if self.focus == 2 => {
                    self.focus = 3;
                    KeyOutcome::Consumed
                }
                _ => self.submit_form(wallet, events),
            },
        }
    }

    fn submit_form(&mut self, wallet: &mut WalletState, events: &EventBus) -> KeyOutcome {
        match self.form_mode {
            FormMode::Add => {
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
                        events
                            .publish(ProviderEvent::ChainChanged(format!("0x{:x}", net.chain_id)));
                        if let Some(i) = wallet
                            .networks()
                            .networks()
                            .iter()
                            .position(|n| n.id == net.id)
                        {
                            self.selected = i;
                        }
                        self.clear_form();
                        self.stage = Stage::List;
                    }
                    Err(e) => self.status = e.user_message(),
                }
            }
            FormMode::Edit => match wallet.update_custom_network(
                &self.edit_network_id,
                self.name.value(),
                self.rpc_url.value(),
                self.symbol.value(),
                self.is_testnet,
            ) {
                Ok(net) => {
                    self.status = format!("Updated {}.", net.name);
                    self.clear_form();
                    self.stage = Stage::List;
                }
                Err(e) => self.status = e.user_message(),
            },
        }
        KeyOutcome::Consumed
    }

    fn clear_form(&mut self) {
        self.name.set_value("");
        self.chain_id.set_value("");
        self.rpc_url.set_value("");
        self.symbol.set_value("");
        self.edit_network_id.clear();
        self.edit_chain_id = 0;
    }
}
