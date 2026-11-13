//! Settings / Net: switch networks; add, edit, or remove custom EVM networks;
//! Agent CDP / autonomy toggles; manage F3 wallets (remove hardware / imports);
//! change vault password; edit Sentient daemon policy via password step-up.
//!
//! Footer `n` / `i` both land here. Built-ins are fixed; customs persist in the vault.
//! Built-in RPC: **`r`**. Custom chains: **`a`** add · **`e`** edit · **`d`** delete.
//! Agent box (below networks): Enter toggles CDP / autonomy.
//! Wallets: **`u`** list · **`d`** delete (confirm; imports need vault password).
//! Password: **`p`** change vault password (current → new → confirm).
//! Sentient policy: **`y`** view/update session policy via signer daemon.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph, Wrap},
    Frame,
};
use secrecy::{ExposeSecret, SecretString};
use tokio::runtime::Handle;
use vaughan_agent::sentient::{AgentSessionPolicy, EnforcementMode};
use vaughan_core::core::{
    is_sentient_profile, AgentAutonomyTier, OperatingMode, SignerClient, WalletState,
};
use vaughan_core::security::encryption::validate_password_policy;
use vaughan_core::security::AccountKind;
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
    /// F3 wallets — remove hardware watches / imported keys (not HD).
    Accounts,
    /// Confirm delete for the selected removable account.
    AccountConfirm,
    /// Vault password before removing an imported key.
    AccountPassword,
    /// Change vault password — current password.
    ChangePasswordCurrent,
    /// Change vault password — new password.
    ChangePasswordNew,
    /// Change vault password — confirm new password.
    ChangePasswordConfirm,
    /// Sentient session policy editor (backed by `vaughan signerd`).
    Policy,
    /// Vault password prompt for sentient `policy_update` (never cached or pre-filled).
    PolicyPassword,
}

const POLICY_FIELDS: &[(&str, &str)] = &[
    (
        "enforcement",
        "enforced | warn-only | disabled (Space toggles)",
    ),
    ("max_position_pct", "integer 1..=100"),
    ("max_approve_pct", "integer 1..=100 or none"),
    ("daily_cap_pct", "integer 1..=100"),
    ("max_slippage_bps", "integer 1..=9999 (< 100%)"),
    ("max_session_gas_wei", "wei integer"),
    ("max_consecutive_errors", "integer >= 1"),
    ("required_rpc_quorum", "integer >= 1"),
    ("allow_transfer_to", "comma-separated 0x addresses"),
    ("acknowledge_unsafe", "true | false (Space toggles)"),
];

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
    /// Account list cursor ([`Stage::Accounts`]).
    account_sel: usize,
    /// Pending delete target (account index + display fields).
    pending_remove_index: u32,
    pending_remove_label: String,
    pending_remove_address: String,
    pending_remove_imported: bool,
    account_password: Input,
    /// Held while collecting the new password (cleared on cancel / success).
    pending_current_password: Option<SecretString>,
    pending_new_password: Option<SecretString>,
    /// Sentient daemon policy draft and digest for optimistic concurrency (`expected_digest`).
    policy_draft: Option<AgentSessionPolicy>,
    policy_digest: String,
    policy_focus: usize,
    policy_value: Input,
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
            account_sel: 0,
            pending_remove_index: 0,
            pending_remove_label: String::new(),
            pending_remove_address: String::new(),
            pending_remove_imported: false,
            account_password: Input::new(true, "vault password"),
            pending_current_password: None,
            pending_new_password: None,
            policy_draft: None,
            policy_digest: String::new(),
            policy_focus: 0,
            policy_value: Input::new(false, "value"),
            status: String::new(),
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);

        match self.stage {
            Stage::List => {
                let networks = wallet.networks();
                let active_id = networks.active_id();
                let net_len = networks.networks().len();
                let net_items: Vec<ListItem> = networks
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

                let agent_style = |offset: usize| {
                    if self.selected == net_len + offset {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else {
                        Style::default()
                    }
                };
                let cdp_row = format!(
                    "   CDP browser control: {}",
                    if wallet.agent_browser_control() {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
                let auto_row = format!(
                    "   Autonomy: {}",
                    match wallet.agent_autonomy_tier() {
                        AgentAutonomyTier::Advisor => "Advisor (manual Connect)",
                        AgentAutonomyTier::Operator => "Operator (auto-connect allowlist)",
                    }
                );
                let agent_items = vec![
                    ListItem::new(Line::from(Span::styled(cdp_row, agent_style(0)))),
                    ListItem::new(Line::from(Span::styled(auto_row, agent_style(1)))),
                ];

                let (mode_label, mode_style) = match wallet.operating_mode() {
                    OperatingMode::SentientTrader => {
                        let policy = crate::sentient_mcp::sentient_policy_line(wallet)
                            .unwrap_or_else(|| "policy: unknown".into());
                        (
                            format!("   Mode: Sentient — {policy} (lock l to switch)"),
                            Style::default().fg(Color::Magenta),
                        )
                    }
                    OperatingMode::AiAssisted => (
                        "   Mode: Advisor · MCP on (lock l to switch)".to_string(),
                        Style::default().fg(Color::Green),
                    ),
                    OperatingMode::HumanOnly => (
                        "   Mode: Human only · MCP off (lock l to switch)".to_string(),
                        Style::default().fg(Color::DarkGray),
                    ),
                };
                let burn_on = vaughan_core::core::assist_any_gate_enabled();
                let tools_unlocked = if burn_on {
                    let dir = vaughan_agent::paths::profile_dir(wallet.path());
                    vaughan_core::core::assist_unlock_cached(&dir)
                } else {
                    true
                };
                let agent_h = if burn_on { 7 } else { 6 };
                let [net_area, agent_area, footer_area] = Layout::vertical([
                    Constraint::Min(4),
                    Constraint::Length(agent_h),
                    Constraint::Length(3),
                ])
                .areas(content);
                let net_inner =
                    brand::render_faded_box(frame, net_area, Some(brand::fade_line(" Networks ")));
                frame.render_widget(List::new(net_items), net_inner);

                let agent_inner =
                    brand::render_faded_box(frame, agent_area, Some(brand::fade_line(" Agent ")));
                let note_h = if burn_on { 2 } else { 1 };
                let [agent_list_a, agent_note_a] =
                    Layout::vertical([Constraint::Length(2), Constraint::Length(note_h)])
                        .areas(agent_inner);
                frame.render_widget(List::new(agent_items), agent_list_a);
                let mut agent_notes = vec![Line::from(Span::styled(mode_label, mode_style))];
                if burn_on {
                    let (tools_line, tools_style) = if tools_unlocked {
                        (
                            "   Tools: unlocked (holding or burn) — Dex/Ag/LP free",
                            Style::default().fg(Color::DarkGray),
                        )
                    } else {
                        (
                            "   Tools: hold ≥13 WZRD (w) — Dex/Ag/LP free",
                            Style::default().fg(Color::Yellow),
                        )
                    };
                    agent_notes.push(Line::from(Span::styled(tools_line, tools_style)));
                }
                frame.render_widget(Paragraph::new(agent_notes), agent_note_a);

                let footer_inner = brand::render_faded_box(
                    frame,
                    footer_area,
                    Some(brand::fade_line(" Shortcuts ")),
                );
                let shortcuts = if is_sentient_wallet(wallet) {
                    "↑↓ move · Enter select · a add · e edit · r RPC · d delete · u wallets · p password · y policy · h udev · k Keys · Esc"
                } else {
                    "↑↓ move · Enter select · a add · e edit · r RPC · d delete · u wallets · p password · h udev · k Keys · Esc"
                };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        shortcuts,
                        Style::default().fg(Color::DarkGray),
                    ))),
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
                        Line::from("  Then: unlock · Ethereum app · c Hardware → 4"),
                        Line::from(""),
                        Line::from(Span::styled(
                            "Trezor — add udev rules",
                            Style::default().fg(Color::Cyan),
                        )),
                        Line::from("  trezor.io/guides/trezorctl/udev-rules"),
                        Line::from("  Then: unlock · confirm Ethereum · c Hardware → 5"),
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
            Stage::Accounts => {
                let accounts = match wallet.accounts() {
                    Ok(a) => a,
                    Err(_) => &[],
                };
                let items: Vec<ListItem> = accounts
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        let kind = match &a.kind {
                            AccountKind::Hd => "HD",
                            AccountKind::Imported => "import",
                            AccountKind::Hardware(h) => h.vendor.as_str(),
                        };
                        let rem = if matches!(a.kind, AccountKind::Hd) {
                            ""
                        } else {
                            "  [d delete]"
                        };
                        let short = short_addr(&a.address);
                        let label = format!("   {kind:<7}  {}  {short}{rem}", a.label);
                        let style = if i == self.account_sel {
                            Style::default().fg(Color::Black).bg(Color::Cyan)
                        } else if matches!(a.kind, AccountKind::Hd) {
                            Style::default().fg(Color::DarkGray)
                        } else {
                            Style::default()
                        };
                        ListItem::new(Line::from(Span::styled(label, style)))
                    })
                    .collect();
                let [list_area, hint_area] =
                    Layout::vertical([Constraint::Min(4), Constraint::Length(3)]).areas(content);
                let inner =
                    brand::render_faded_box(frame, list_area, Some(brand::fade_line(" Wallets ")));
                frame.render_widget(List::new(items), inner);
                let hint_inner = brand::render_faded_box(frame, hint_area, None);
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "↑↓ select · d delete watch/import · Esc back · HD rows stay (seed)",
                        Style::default().fg(Color::DarkGray),
                    ))),
                    hint_inner,
                );
            }
            Stage::AccountConfirm => {
                let short = short_addr(&self.pending_remove_address);
                let kind = if self.pending_remove_imported {
                    "imported key"
                } else {
                    "hardware watch (device unchanged)"
                };
                let inner = brand::render_faded_box(
                    frame,
                    content,
                    Some(brand::fade_line(" Remove wallet? ")),
                );
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(format!(
                            "Remove «{}» ({short}) from Vaughan?",
                            self.pending_remove_label
                        )),
                        Line::from(Span::styled(kind, Style::default().fg(Color::DarkGray))),
                        Line::from(""),
                        Line::from(Span::styled(
                            "y confirm · n / Esc cancel",
                            Style::default().fg(Color::Yellow),
                        )),
                    ])
                    .wrap(Wrap { trim: false }),
                    inner,
                );
            }
            Stage::AccountPassword => {
                let [msg, pw] =
                    Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(content);
                let msg_inner = brand::render_faded_box(
                    frame,
                    msg,
                    Some(brand::fade_line(" Confirm vault password ")),
                );
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(format!(
                            "Removing imported «{}» rewrites the vault.",
                            self.pending_remove_label
                        )),
                        Line::from(Span::styled(
                            "Enter password · Esc cancel",
                            Style::default().fg(Color::DarkGray),
                        )),
                    ]),
                    msg_inner,
                );
                render_labeled_input(frame, pw, "Password", &self.account_password, true);
            }
            Stage::ChangePasswordCurrent
            | Stage::ChangePasswordNew
            | Stage::ChangePasswordConfirm => {
                let (title, hint, label) = match self.stage {
                    Stage::ChangePasswordCurrent => (
                        " Change vault password ",
                        "Enter current password · Esc cancel",
                        "Current password",
                    ),
                    Stage::ChangePasswordNew => (
                        " Choose new password ",
                        "≥12 chars with upper, lower, digit, symbol · Esc cancel",
                        "New password",
                    ),
                    _ => (
                        " Confirm new password ",
                        "Re-enter to confirm · Enter saves (a few seconds) · Esc cancel",
                        "Confirm password",
                    ),
                };
                let [msg, pw] =
                    Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(content);
                let msg_inner = brand::render_faded_box(frame, msg, Some(brand::fade_line(title)));
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(
                            "Rotates the vault encryption password (and Piteas key if saved).",
                        ),
                        Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray))),
                    ]),
                    msg_inner,
                );
                render_labeled_input(frame, pw, label, &self.account_password, true);
            }
            Stage::Policy => {
                let [hdr_area, list_area, edit_area] = Layout::vertical([
                    Constraint::Length(4),
                    Constraint::Min(6),
                    Constraint::Length(3),
                ])
                .areas(content);
                let digest_short = if self.policy_digest.len() > 12 {
                    &self.policy_digest[..12]
                } else {
                    &self.policy_digest
                };
                let hdr_inner = brand::render_faded_box(
                    frame,
                    hdr_area,
                    Some(brand::fade_line(" Sentient policy (signerd) ")),
                );
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(format!("Digest: {digest_short}")),
                        Line::from(Span::styled(
                            "↑↓ select · edit value · Space toggle · Enter apply (vault password) · Esc back",
                            Style::default().fg(Color::DarkGray),
                        )),
                    ]),
                    hdr_inner,
                );

                let items: Vec<ListItem> = match &self.policy_draft {
                    Some(policy) => POLICY_FIELDS
                        .iter()
                        .enumerate()
                        .map(|(i, (name, _hint))| {
                            let val = policy_field_display(policy, i);
                            let line_text = if val.is_empty() {
                                format!("  {name:<24} (empty)")
                            } else {
                                format!("  {name:<24} {val}")
                            };
                            let style = if i == self.policy_focus {
                                Style::default().fg(Color::Black).bg(Color::Cyan)
                            } else {
                                Style::default()
                            };
                            ListItem::new(Line::from(Span::styled(line_text, style)))
                        })
                        .collect(),
                    None => vec![ListItem::new(Line::from("  No policy loaded"))],
                };
                let list_inner = brand::render_faded_box(frame, list_area, None);
                frame.render_widget(List::new(items), list_inner);

                let (field_name, field_hint) =
                    POLICY_FIELDS[self.policy_focus.min(POLICY_FIELDS.len() - 1)];
                let label = format!("{field_name} ({field_hint})");
                render_labeled_input(frame, edit_area, &label, &self.policy_value, true);
            }
            Stage::PolicyPassword => {
                let [msg, pw] =
                    Layout::vertical([Constraint::Min(4), Constraint::Length(3)]).areas(content);
                let msg_inner = brand::render_faded_box(
                    frame,
                    msg,
                    Some(brand::fade_line(" Confirm policy update (step-up) ")),
                );
                let summary = self
                    .policy_draft
                    .as_ref()
                    .map(|p| {
                        format!(
                            "enforcement={}  max_position={}  daily_cap={}  slippage={}bps  unsafe={}",
                            p.enforcement.as_str(),
                            p.max_position_pct,
                            p.daily_cap_pct,
                            p.max_slippage_bps,
                            p.acknowledge_unsafe,
                        )
                    })
                    .unwrap_or_default();
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(summary),
                        Line::from(
                            "Enter vault password to authorize signer daemon policy_update (never cached).",
                        ),
                        Line::from(Span::styled(
                            "Enter submit · Esc cancel back to policy fields",
                            Style::default().fg(Color::DarkGray),
                        )),
                    ])
                    .wrap(Wrap { trim: false }),
                    msg_inner,
                );
                render_labeled_input(frame, pw, "Vault password", &self.account_password, true);
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
        events: &EventBus,
    ) -> KeyOutcome {
        match self.stage {
            Stage::List => self.handle_list_key(key, wallet, handle, events),
            Stage::Form => self.handle_form_key(key, wallet, events),
            Stage::HardwareHelp => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::Consumed,
            },
            Stage::RpcPick => self.handle_rpc_pick_key(key, wallet),
            Stage::Accounts => self.handle_accounts_key(key, wallet),
            Stage::AccountConfirm => self.handle_account_confirm_key(key, wallet, events),
            Stage::AccountPassword => self.handle_account_password_key(key, wallet, events),
            Stage::ChangePasswordCurrent
            | Stage::ChangePasswordNew
            | Stage::ChangePasswordConfirm => self.handle_change_password_key(key, wallet),
            Stage::Policy => self.handle_policy_key(key),
            Stage::PolicyPassword => self.handle_policy_password_key(key, handle),
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
        handle: &Handle,
        events: &EventBus,
    ) -> KeyOutcome {
        let net_len = wallet.networks().networks().len();
        let len = net_len + 2; // + CDP toggle row, + autonomy toggle row
        match key.code {
            KeyCode::Esc => KeyOutcome::Back,
            KeyCode::Char('k') => KeyOutcome::Navigate(Screen::Keys),
            KeyCode::Char('q') => KeyOutcome::Navigate(Screen::Dapps),
            KeyCode::Char('c') => KeyOutcome::Navigate(Screen::Browser),
            KeyCode::Char('h') => {
                self.stage = Stage::HardwareHelp;
                self.status.clear();
                KeyOutcome::Consumed
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if is_sentient_wallet(wallet) {
                    self.open_policy(handle)
                } else {
                    self.status =
                        "Policy step-up is only available on the sentient profile.".into();
                    KeyOutcome::Consumed
                }
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                self.account_sel = 0;
                self.stage = Stage::Accounts;
                self.status = "Select a wallet · d removes hardware watches / imports".into();
                KeyOutcome::Consumed
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                self.begin_change_password();
                KeyOutcome::Consumed
            }
            KeyCode::Char('w') | KeyCode::Char('W') => {
                if vaughan_core::core::assist_burn_gate_enabled() {
                    KeyOutcome::AssistBurn
                } else {
                    self.status =
                        "Tools gates disabled — unset VAUGHAN_ASSIST_HOLD_GATE / VAUGHAN_ASSIST_BURN_GATE or remove =0."
                            .into();
                    KeyOutcome::Handled
                }
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
                if self.selected >= net_len {
                    self.status = "Select a network row to edit RPC/name settings.".into();
                    return KeyOutcome::Handled;
                }
                let Some(net) = wallet.networks().networks().get(self.selected) else {
                    return KeyOutcome::Handled;
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
                if self.selected >= net_len {
                    self.status = "Select a network row first.".into();
                    return KeyOutcome::Handled;
                }
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
                if self.selected == net_len {
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
                } else if self.selected == net_len + 1 {
                    let next = match wallet.agent_autonomy_tier() {
                        AgentAutonomyTier::Advisor => AgentAutonomyTier::Operator,
                        AgentAutonomyTier::Operator => AgentAutonomyTier::Advisor,
                    };
                    match wallet.set_agent_autonomy_tier(next) {
                        Ok(()) => {
                            self.status = match next {
                                AgentAutonomyTier::Operator => {
                                    "Operator tier — auto-connect on trusted dApp / Ag hosts (sign still manual)."
                                        .into()
                                }
                                AgentAutonomyTier::Advisor => {
                                    "Advisor tier — manual Connect approval for every site.".into()
                                }
                            };
                        }
                        Err(e) => self.status = e.user_message(),
                    }
                } else if let Some(net) = wallet.networks().networks().get(self.selected) {
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
                if self.selected >= net_len {
                    self.status = "Only custom network rows can be deleted.".into();
                    return KeyOutcome::Handled;
                }
                let Some(net) = wallet.networks().networks().get(self.selected).cloned() else {
                    return KeyOutcome::Handled;
                };
                if !wallet.networks().is_custom(&net.id) {
                    self.status = "Built-in networks cannot be removed.".into();
                    return KeyOutcome::Handled;
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
                KeyOutcome::Handled
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

    fn handle_accounts_key(&mut self, key: KeyEvent, wallet: &WalletState) -> KeyOutcome {
        let len = wallet.accounts().map(|a| a.len()).unwrap_or(0);
        match key.code {
            KeyCode::Esc => {
                self.stage = Stage::List;
                self.status.clear();
                KeyOutcome::Consumed
            }
            KeyCode::Up => {
                self.account_sel = self.account_sel.saturating_sub(1);
                KeyOutcome::Consumed
            }
            KeyCode::Down if len > 0 => {
                self.account_sel = (self.account_sel + 1).min(len - 1);
                KeyOutcome::Consumed
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                let Ok(accounts) = wallet.accounts() else {
                    self.status = "Unlock the vault first.".into();
                    return KeyOutcome::Consumed;
                };
                let Some(account) = accounts.get(self.account_sel) else {
                    return KeyOutcome::Consumed;
                };
                if matches!(account.kind, AccountKind::Hd) {
                    self.status =
                        "HD wallets come from the vault seed — they cannot be removed.".into();
                    return KeyOutcome::Consumed;
                }
                self.pending_remove_index = account.index;
                self.pending_remove_label = account.label.clone();
                self.pending_remove_address = account.address.clone();
                self.pending_remove_imported = account.is_imported;
                self.stage = Stage::AccountConfirm;
                self.status.clear();
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::Consumed,
        }
    }

    fn handle_account_confirm_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        events: &EventBus,
    ) -> KeyOutcome {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.stage = Stage::Accounts;
                self.status = "Delete cancelled.".into();
                KeyOutcome::Consumed
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if self.pending_remove_imported {
                    self.account_password.set_value("");
                    self.stage = Stage::AccountPassword;
                    self.status.clear();
                    return KeyOutcome::Consumed;
                }
                self.finish_hardware_remove(wallet, events)
            }
            _ => KeyOutcome::Consumed,
        }
    }

    fn handle_account_password_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        events: &EventBus,
    ) -> KeyOutcome {
        if key.code == KeyCode::Esc {
            let _ = self.account_password.take_secret();
            self.stage = Stage::Accounts;
            self.status = "Delete cancelled.".into();
            return KeyOutcome::Consumed;
        }
        match self.account_password.handle_key(key) {
            InputAction::Ignored => KeyOutcome::NotHandled,
            InputAction::Consumed => KeyOutcome::Consumed,
            InputAction::Submitted => {
                let pw = self.account_password.take_secret();
                match wallet.remove_imported_account(&pw, self.pending_remove_index) {
                    Ok(removed) => {
                        self.after_account_removed(wallet, events, &removed.label, &removed.address)
                    }
                    Err(e) => {
                        self.status = e.user_message();
                        KeyOutcome::Consumed
                    }
                }
            }
        }
    }

    fn finish_hardware_remove(
        &mut self,
        wallet: &mut WalletState,
        events: &EventBus,
    ) -> KeyOutcome {
        match wallet.remove_hardware_account(self.pending_remove_index) {
            Ok(removed) => {
                self.after_account_removed(wallet, events, &removed.label, &removed.address)
            }
            Err(e) => {
                self.status = e.user_message();
                self.stage = Stage::Accounts;
                KeyOutcome::Consumed
            }
        }
    }

    fn after_account_removed(
        &mut self,
        wallet: &WalletState,
        events: &EventBus,
        label: &str,
        address: &str,
    ) -> KeyOutcome {
        let short = short_addr(address);
        self.status = format!("Removed {label} ({short}) from Vaughan.");
        self.account_sel = self.account_sel.saturating_sub(1);
        self.stage = Stage::Accounts;
        if let Ok(addr) = wallet.active_address() {
            events.publish(ProviderEvent::AccountsChanged(vec![addr.to_string()]));
        }
        KeyOutcome::AccountListChanged
    }

    fn begin_change_password(&mut self) {
        self.clear_change_password();
        self.account_password = Input::new(true, "current password");
        self.stage = Stage::ChangePasswordCurrent;
        self.status.clear();
    }

    fn clear_change_password(&mut self) {
        let _ = self.account_password.take_secret();
        self.pending_current_password = None;
        self.pending_new_password = None;
    }

    fn handle_change_password_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
    ) -> KeyOutcome {
        if key.code == KeyCode::Esc {
            self.clear_change_password();
            self.stage = Stage::List;
            self.status = "Password change cancelled.".into();
            return KeyOutcome::Consumed;
        }
        match self.account_password.handle_key(key) {
            InputAction::Ignored => KeyOutcome::NotHandled,
            InputAction::Consumed => KeyOutcome::Consumed,
            InputAction::Submitted => match self.stage {
                Stage::ChangePasswordCurrent => {
                    let current = self.account_password.take_secret();
                    match wallet.verify_password(&current) {
                        Ok(()) => {
                            self.pending_current_password = Some(current);
                            self.account_password = Input::new(true, "new password");
                            self.stage = Stage::ChangePasswordNew;
                            self.status.clear();
                        }
                        Err(e) => {
                            self.status = e.user_message();
                            self.account_password = Input::new(true, "current password");
                        }
                    }
                    KeyOutcome::Consumed
                }
                Stage::ChangePasswordNew => {
                    let new_pw = self.account_password.take_secret();
                    match validate_password_policy(&new_pw) {
                        Ok(()) => {
                            if self
                                .pending_current_password
                                .as_ref()
                                .is_some_and(|c| c.expose_secret() == new_pw.expose_secret())
                            {
                                self.status =
                                    "New password must differ from the current password.".into();
                                self.account_password = Input::new(true, "new password");
                            } else {
                                self.pending_new_password = Some(new_pw);
                                self.account_password = Input::new(true, "confirm password");
                                self.stage = Stage::ChangePasswordConfirm;
                                self.status.clear();
                            }
                        }
                        Err(e) => {
                            self.status = e.user_message();
                            self.account_password = Input::new(true, "new password");
                        }
                    }
                    KeyOutcome::Consumed
                }
                Stage::ChangePasswordConfirm => {
                    let confirm = self.account_password.take_secret();
                    let matches = self
                        .pending_new_password
                        .as_ref()
                        .is_some_and(|p| p.expose_secret() == confirm.expose_secret());
                    if !matches {
                        self.pending_new_password = None;
                        self.account_password = Input::new(true, "new password");
                        self.stage = Stage::ChangePasswordNew;
                        self.status = "Passwords do not match.".into();
                        return KeyOutcome::Consumed;
                    }
                    let Some(current) = self.pending_current_password.take() else {
                        self.clear_change_password();
                        self.stage = Stage::List;
                        self.status = "Password change cancelled.".into();
                        return KeyOutcome::Consumed;
                    };
                    let Some(new_pw) = self.pending_new_password.take() else {
                        self.clear_change_password();
                        self.stage = Stage::List;
                        self.status = "Password change cancelled.".into();
                        return KeyOutcome::Consumed;
                    };
                    match wallet.change_password(&current, &new_pw) {
                        Ok(()) => {
                            self.clear_change_password();
                            self.stage = Stage::List;
                            self.status =
                                "Vault password updated — use the new password next unlock.".into();
                            // Chrome toast: the list status line alone was easy to miss
                            // (and used to render in error-red).
                            return KeyOutcome::Flash(
                                "Password changed successfully. Unlock with the new password next time."
                                    .into(),
                            );
                        }
                        Err(e) => {
                            self.pending_current_password = Some(current);
                            self.pending_new_password = None;
                            self.account_password = Input::new(true, "new password");
                            self.stage = Stage::ChangePasswordNew;
                            self.status = e.user_message();
                        }
                    }
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::Consumed,
            },
        }
    }

    fn clear_form(&mut self) {
        self.name.set_value("");
        self.chain_id.set_value("");
        self.rpc_url.set_value("");
        self.symbol.set_value("");
        self.edit_network_id.clear();
        self.edit_chain_id = 0;
    }

    fn clear_policy_state(&mut self) {
        let _ = self.account_password.take_secret();
        self.account_password = Input::new(true, "vault password");
        self.policy_draft = None;
        self.policy_digest.clear();
        self.policy_focus = 0;
        self.policy_value.set_value("");
    }

    fn open_policy(&mut self, handle: &Handle) -> KeyOutcome {
        let client = SignerClient::default();
        let resp = match block_on_handle(handle, client.policy()) {
            Ok(r) => r,
            Err(e) => {
                self.status = format!("Policy load failed: {e}");
                return KeyOutcome::Consumed;
            }
        };
        let Some(policy_val) = resp.policy else {
            self.status = "Signer daemon returned no policy payload.".into();
            return KeyOutcome::Consumed;
        };
        let policy: AgentSessionPolicy = match serde_json::from_value(policy_val) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("Policy decode failed: {e}");
                return KeyOutcome::Consumed;
            }
        };
        self.policy_digest = resp.policy_digest.unwrap_or_default();
        self.policy_draft = Some(policy);
        self.policy_focus = 0;
        self.sync_policy_input();
        // Never pre-fill or carry over any previous password input.
        let _ = self.account_password.take_secret();
        self.account_password = Input::new(true, "vault password");
        self.stage = Stage::Policy;
        self.status.clear();
        KeyOutcome::Consumed
    }

    fn sync_policy_input(&mut self) {
        if let Some(policy) = &self.policy_draft {
            let val = policy_field_display(policy, self.policy_focus);
            self.policy_value = Input::new(false, "value");
            self.policy_value.set_value(&val);
        }
    }

    fn commit_policy_field(&mut self) -> Result<(), String> {
        let Some(draft) = self.policy_draft.as_mut() else {
            return Err("No policy loaded.".into());
        };
        let (key, _) = POLICY_FIELDS[self.policy_focus.min(POLICY_FIELDS.len() - 1)];
        draft
            .set_field(key, self.policy_value.value())
            .map_err(|e| e.to_string())
    }

    fn move_policy_focus(&mut self, delta: isize) -> KeyOutcome {
        if let Err(e) = self.commit_policy_field() {
            self.status = e;
            return KeyOutcome::Consumed;
        }
        let len = POLICY_FIELDS.len() as isize;
        let next = (self.policy_focus as isize + delta).rem_euclid(len) as usize;
        self.policy_focus = next;
        self.sync_policy_input();
        self.status.clear();
        KeyOutcome::Consumed
    }

    fn handle_policy_key(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                self.clear_policy_state();
                self.stage = Stage::List;
                self.status = "Policy edit cancelled.".into();
                KeyOutcome::Consumed
            }
            KeyCode::Up | KeyCode::BackTab => self.move_policy_focus(-1),
            KeyCode::Down | KeyCode::Tab => self.move_policy_focus(1),
            KeyCode::Char(' ') if self.policy_focus == 0 || self.policy_focus == 9 => {
                if let Some(draft) = self.policy_draft.as_mut() {
                    if self.policy_focus == 0 {
                        draft.enforcement = match draft.enforcement {
                            EnforcementMode::Enforced => EnforcementMode::WarnOnly,
                            EnforcementMode::WarnOnly => EnforcementMode::Disabled,
                            EnforcementMode::Disabled => EnforcementMode::Enforced,
                        };
                    } else {
                        draft.acknowledge_unsafe = !draft.acknowledge_unsafe;
                    }
                    self.sync_policy_input();
                    self.status.clear();
                }
                KeyOutcome::Consumed
            }
            _ => match self.policy_value.handle_key(key) {
                InputAction::Ignored => KeyOutcome::NotHandled,
                InputAction::Consumed => KeyOutcome::Consumed,
                InputAction::Submitted => {
                    if let Err(e) = self.commit_policy_field() {
                        self.status = e;
                        return KeyOutcome::Consumed;
                    }
                    if let Some(draft) = &self.policy_draft {
                        if let Err(e) = draft.validate() {
                            self.status = e.to_string();
                            return KeyOutcome::Consumed;
                        }
                    }
                    let _ = self.account_password.take_secret();
                    self.account_password = Input::new(true, "vault password");
                    self.stage = Stage::PolicyPassword;
                    self.status.clear();
                    KeyOutcome::Consumed
                }
            },
        }
    }

    fn handle_policy_password_key(&mut self, key: KeyEvent, handle: &Handle) -> KeyOutcome {
        if key.code == KeyCode::Esc {
            let _ = self.account_password.take_secret();
            self.account_password = Input::new(true, "vault password");
            self.stage = Stage::Policy;
            self.status = "Password entry cancelled.".into();
            return KeyOutcome::Consumed;
        }
        match self.account_password.handle_key(key) {
            InputAction::Ignored => KeyOutcome::NotHandled,
            InputAction::Consumed => KeyOutcome::Consumed,
            InputAction::Submitted => {
                let pw = self.account_password.take_secret();
                // Always reset immediately so the password is never cached or pre-filled.
                self.account_password = Input::new(true, "vault password");
                let Some(draft) = self.policy_draft.as_ref() else {
                    drop(pw);
                    self.stage = Stage::List;
                    self.status = "Policy draft lost.".into();
                    return KeyOutcome::Consumed;
                };
                let policy_json = match serde_json::to_value(draft) {
                    Ok(v) => v,
                    Err(e) => {
                        drop(pw);
                        self.stage = Stage::Policy;
                        self.status = format!("Policy encode error: {e}");
                        return KeyOutcome::Consumed;
                    }
                };
                let expected = if self.policy_digest.is_empty() {
                    None
                } else {
                    Some(self.policy_digest.clone())
                };
                let client = SignerClient::default();
                let res = block_on_handle(handle, client.policy_update(pw, policy_json, expected));
                match res {
                    Ok(resp) => {
                        let new_digest = resp.policy_digest.unwrap_or_default();
                        self.clear_policy_state();
                        self.stage = Stage::List;
                        self.status = format!("Policy updated on signerd (digest {new_digest}).");
                        KeyOutcome::Flash("Sentient policy updated.".into())
                    }
                    Err(e) => {
                        if e.contains("step_up_") {
                            self.status = e;
                        } else {
                            self.stage = Stage::Policy;
                            self.status = e;
                        }
                        KeyOutcome::Consumed
                    }
                }
            }
        }
    }
}

fn is_sentient_wallet(wallet: &WalletState) -> bool {
    wallet.operating_mode() == OperatingMode::SentientTrader
        || wallet
            .path()
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .is_some_and(is_sentient_profile)
}

fn policy_field_display(policy: &AgentSessionPolicy, idx: usize) -> String {
    match idx {
        0 => policy.enforcement.as_str().to_string(),
        1 => policy.max_position_pct.to_string(),
        2 => policy
            .max_approve_pct
            .map(|p| p.to_string())
            .unwrap_or_else(|| "none".to_string()),
        3 => policy.daily_cap_pct.to_string(),
        4 => policy.max_slippage_bps.to_string(),
        5 => policy.max_session_gas_wei.clone(),
        6 => policy.max_consecutive_errors.to_string(),
        7 => policy.required_rpc_quorum.to_string(),
        8 => policy.allow_transfer_to.join(", "),
        _ => policy.acknowledge_unsafe.to_string(),
    }
}

fn block_on_handle<F, T>(handle: &Handle, fut: F) -> T
where
    F: std::future::Future<Output = T> + Send,
    T: Send,
{
    match Handle::try_current() {
        Ok(cur) => match cur.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| handle.block_on(fut))
            }
            _ => std::thread::scope(|s| {
                s.spawn(|| handle.block_on(fut))
                    .join()
                    .expect("policy rpc thread panicked")
            }),
        },
        Err(_) => handle.block_on(fut),
    }
}

fn short_addr(address: &str) -> String {
    let a = address.trim();
    if a.len() >= 10 {
        format!("{}…{}", &a[..6], &a[a.len() - 4..])
    } else {
        a.to_string()
    }
}
