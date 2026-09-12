//! Keys: export recovery phrase / private key, import hex key, add Ledger/Trezor.
//!
//! Every reveal path re-checks the vault password. Secrets are shown once and
//! cleared when the user leaves the screen — never logged.
//!
//! Private-key export always uses the **F3-active** account (the account shown
//! in the status strip). Recovery phrase is the vault HD seed (all HD wallets).
//! Hardware accounts cannot export keys; use options 4–5 to add a device watch.

use std::sync::mpsc::{self, Receiver};

use crate::app::KeyOutcome;
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::spinner_frame;
use crate::views::{body_areas, render_labeled_input, status_paragraph};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use secrecy::{ExposeSecret, SecretString};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::{EventBus, ProviderEvent};

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuItem {
    ExportPhrase,
    ExportKey,
    ImportKey,
    AddLedger,
    AddTrezor,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Menu,
    /// Footer `c` Hardware — pick Ledger vs Trezor.
    HardwareHub,
    Password,
    Reveal,
    ImportForm,
    /// Trezor USB worker in flight (PIN matrix handled by App overlay).
    DeviceBusy,
    DevicePick,
}

/// Background USB result channels (Trezor must not `block_on` the UI thread).
enum DeviceJob {
    Idle,
    Preview(Receiver<Result<Vec<(String, String)>, String>>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeviceVendor {
    Ledger,
    Trezor,
}

pub struct KeysView {
    stage: Stage,
    menu: MenuItem,
    password: Input,
    /// Password confirmed for the current import flow (never logged).
    verified_password: Option<SecretString>,
    label: Input,
    private_key: Input,
    import_focus: usize,
    /// Revealed secret for display only; zeroized on drop (SecretString) and
    /// cleared on leave.
    revealed: Option<SecretString>,
    reveal_title: String,
    status: String,
    /// Device path preview: (path, address).
    device_paths: Vec<(String, String)>,
    device_sel: usize,
    device_vendor: DeviceVendor,
    device_job: DeviceJob,
    /// Animation tick while [`Stage::DeviceBusy`].
    busy_tick: u64,
}

impl Default for KeysView {
    fn default() -> Self {
        Self {
            stage: Stage::Menu,
            menu: MenuItem::ExportPhrase,
            password: Input::new(true, "vault password"),
            verified_password: None,
            label: Input::new(false, "optional — blank → Wn-HD k"),
            private_key: Input::new(true, "0x… private key"),
            import_focus: 0,
            revealed: None,
            reveal_title: String::new(),
            status: String::new(),
            device_paths: Vec::new(),
            device_sel: 0,
            device_vendor: DeviceVendor::Ledger,
            device_job: DeviceJob::Idle,
            busy_tick: 0,
        }
    }
}

impl KeysView {
    /// Footer **Hardware** chip (`c`) — Ledger / Trezor hub (skips export menu).
    pub fn hardware_hub() -> Self {
        let mut v = Self::default();
        v.stage = Stage::HardwareHub;
        v.menu = MenuItem::AddLedger;
        v
    }

    fn clear_secret(&mut self) {
        // SecretString zeroizes on drop.
        self.revealed = None;
        self.verified_password = None;
        self.password.set_value("");
        self.private_key.set_value("");
    }

    /// Menu digit / Enter → password or device-ready gate.
    fn begin_menu_action(&mut self) {
        self.status.clear();
        self.password.set_value("");
        self.stage = Stage::Password;
    }

    /// F3-active account line for Keys copy (label + short address).
    fn f3_account_line(wallet: &WalletState) -> String {
        match wallet.active_account_export_context() {
            Ok((label, address, imported)) => {
                let kind = if wallet.active_is_hardware().unwrap_or(false) {
                    "hardware"
                } else if imported {
                    "imported"
                } else {
                    "HD"
                };
                format!("F3: {label} ({kind}) · {}", short_addr(&address))
            }
            Err(_) => "F3: —".into(),
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let f3 = Self::f3_account_line(wallet);

        if self.stage == Stage::Reveal {
            self.render_reveal(frame, content, &f3);
            frame.render_widget(status_paragraph(&self.status), status_area);
            return;
        }

        let text = match self.stage {
            Stage::Menu => vec![
                Line::from("Keys — export / import / hardware (password for secrets)"),
                Line::from(Span::styled(f3.clone(), Style::default().fg(Color::Cyan))),
                Line::from(""),
                menu_line(
                    self.menu == MenuItem::ExportPhrase,
                    "1  Export vault recovery phrase (HD seed)",
                ),
                menu_line(
                    self.menu == MenuItem::ExportKey,
                    "2  Export F3 wallet private key",
                ),
                menu_line(self.menu == MenuItem::ImportKey, "3  Import private key"),
                menu_line(
                    self.menu == MenuItem::AddLedger,
                    "4  Add Ledger (USB · Ethereum app)",
                ),
                menu_line(
                    self.menu == MenuItem::AddTrezor,
                    "5  Add Trezor (USB · confirm Ethereum)",
                ),
                Line::from(""),
                Line::from("1–5 — open item   ↑↓ — highlight   Enter — open   Esc — back"),
                Line::from(Span::styled(
                    "Tip: footer c Hardware jumps straight here for devices",
                    Style::default().fg(Color::DarkGray),
                )),
            ],
            Stage::HardwareHub => vec![
                Line::from(Span::styled(
                    "Hardware — add a device watch (keys stay on device)",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(f3.clone(), Style::default().fg(Color::Cyan))),
                Line::from(""),
                menu_line(
                    self.menu == MenuItem::AddLedger,
                    "4  Add Ledger (USB · Ethereum app)",
                ),
                menu_line(
                    self.menu == MenuItem::AddTrezor,
                    "5  Add Trezor (USB · confirm Ethereum)",
                ),
                Line::from(""),
                Line::from("4 / 5 — open   ↑↓ — highlight   Enter — open   Esc — back"),
                Line::from(Span::styled(
                    "Linux USB: Settings (n) → h — udev help",
                    Style::default().fg(Color::DarkGray),
                )),
            ],
            Stage::Password => vec![
                Line::from(Span::styled(f3, Style::default().fg(Color::Cyan))),
                Line::from(""),
                Line::from(match self.menu {
                    MenuItem::ExportPhrase => {
                        "Re-enter vault password to show recovery phrase (all HD wallets)"
                    }
                    MenuItem::ExportKey => {
                        "Re-enter vault password to show this F3 wallet's private key"
                    }
                    MenuItem::ImportKey => "Re-enter vault password to import a key",
                    MenuItem::AddLedger => "Unlock device, open Ethereum app, then continue",
                    MenuItem::AddTrezor => "Plug in Trezor, then continue to connect",
                }),
                Line::from(""),
                Line::from("Esc — cancel"),
            ],
            Stage::Reveal => unreachable!("handled above"),
            Stage::ImportForm => vec![
                Line::from("Import a hex private key into this vault"),
                Line::from("Tab — next field   Enter — import   Esc — cancel"),
            ],
            Stage::DeviceBusy => vec![
                Line::from(format!(
                    "{} Connecting to Trezor…",
                    spinner_frame(self.busy_tick)
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Use the blank PIN pad overlay (digits only on the device).",
                    Style::default().fg(Color::Yellow),
                )),
                Line::from("↑↓←→ move · Space select · Enter/s submit · Esc cancel"),
                Line::from(""),
                Line::from(Span::styled(
                    "Model T / Safe: unlock on the touchscreen.",
                    Style::default().fg(Color::DarkGray),
                )),
            ],
            Stage::DevicePick => {
                let vendor = match self.device_vendor {
                    DeviceVendor::Ledger => "Ledger",
                    DeviceVendor::Trezor => "Trezor",
                };
                let mut lines = vec![
                    Line::from(Span::styled(
                        format!(
                            "Confirm on {vendor} if prompted · ↑↓ pick · Enter add · Esc cancel"
                        ),
                        Style::default().fg(Color::Yellow),
                    )),
                    Line::from(""),
                ];
                if self.device_paths.is_empty() {
                    lines.push(Line::from(format!(
                        "No paths — check USB / {vendor} ready."
                    )));
                } else {
                    for (i, (path, addr)) in self.device_paths.iter().enumerate() {
                        lines.push(menu_line(
                            i == self.device_sel,
                            &format!("{path}  {}", short_addr(addr)),
                        ));
                    }
                }
                lines
            }
        };

        match self.stage {
            Stage::Password if matches!(self.menu, MenuItem::AddLedger | MenuItem::AddTrezor) => {
                let title = match self.menu {
                    MenuItem::AddTrezor => " Trezor ",
                    _ => " Ledger ",
                };
                let lines = match self.menu {
                    MenuItem::AddTrezor => vec![
                        Line::from(Span::styled(
                            "Add a Trezor wallet",
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        )),
                        Line::from(""),
                        Line::from("1. Plug the Trezor into USB (use a data cable, not charge-only)."),
                        Line::from("2. Wake the device — press the button if the screen is blank."),
                        Line::from("3. Press Enter here to connect."),
                        Line::from(""),
                        Line::from(Span::styled(
                            "Trezor One",
                            Style::default().add_modifier(Modifier::BOLD),
                        )),
                        Line::from(
                            "  A blank pad opens on connect. Digits stay on the Trezor (scrambled).",
                        ),
                        Line::from(
                            "  ↑↓←→ matching cell · Space select · Enter/s submit.",
                        ),
                        Line::from(""),
                        Line::from(Span::styled(
                            "Model T / Safe",
                            Style::default().add_modifier(Modifier::BOLD),
                        )),
                        Line::from("  Unlock with PIN on the device touchscreen."),
                        Line::from(""),
                        Line::from(Span::styled(
                            "If connect fails on Linux: Settings (n) → h for USB udev help.",
                            Style::default().fg(Color::DarkGray),
                        )),
                        Line::from("Enter — connect   Esc — cancel"),
                    ],
                    _ => vec![
                        Line::from(Span::styled(
                            "Add a Ledger wallet",
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        )),
                        Line::from(""),
                        Line::from("1. Plug the Ledger into USB."),
                        Line::from("2. Unlock it and open the Ethereum app."),
                        Line::from("3. Press Enter here to connect."),
                        Line::from(""),
                        Line::from(Span::styled(
                            "If connect fails on Linux: Settings (n) → h for USB udev help.",
                            Style::default().fg(Color::DarkGray),
                        )),
                        Line::from("Enter — connect   Esc — cancel"),
                    ],
                };
                let inner = brand::render_faded_box(frame, content, Some(brand::fade_line(title)));
                frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
            }
            Stage::Password => {
                let [msg, pw] =
                    ratatui::layout::Layout::vertical([Constraint::Min(3), Constraint::Length(3)])
                        .areas(content);
                let msg_inner =
                    brand::render_faded_box(frame, msg, Some(brand::fade_line(" Keys ")));
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), msg_inner);
                render_labeled_input(frame, pw, "Password", &self.password, true);
            }
            Stage::ImportForm => {
                let [msg, label_a, key_a] = ratatui::layout::Layout::vertical([
                    Constraint::Min(2),
                    Constraint::Length(3),
                    Constraint::Length(3),
                ])
                .areas(content);
                let msg_inner =
                    brand::render_faded_box(frame, msg, Some(brand::fade_line(" Import key ")));
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), msg_inner);
                render_labeled_input(frame, label_a, "Label", &self.label, self.import_focus == 0);
                render_labeled_input(
                    frame,
                    key_a,
                    "Private key",
                    &self.private_key,
                    self.import_focus == 1,
                );
            }
            Stage::HardwareHub => {
                let inner =
                    brand::render_faded_box(frame, content, Some(brand::fade_line(" Hardware ")));
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            }
            Stage::DevicePick => {
                let title = match self.device_vendor {
                    DeviceVendor::Ledger => " Ledger ",
                    DeviceVendor::Trezor => " Trezor ",
                };
                let inner = brand::render_faded_box(frame, content, Some(brand::fade_line(title)));
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            }
            Stage::DeviceBusy => {
                let inner =
                    brand::render_faded_box(frame, content, Some(brand::fade_line(" Trezor ")));
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            }
            _ => {
                let inner =
                    brand::render_faded_box(frame, content, Some(brand::fade_line(" Keys ")));
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            }
        }
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    /// Secret sits on its own unbordered rows so mouse-select / copy won't
    /// pick up box-drawing characters.
    fn render_reveal(&self, frame: &mut Frame, area: Rect, f3: &str) {
        let [header, secret_area, footer] = ratatui::layout::Layout::vertical([
            Constraint::Length(5),
            Constraint::Min(3),
            Constraint::Length(4),
        ])
        .spacing(0)
        .areas(area);

        let head_inner = brand::render_faded_box(frame, header, Some(brand::fade_line(" Keys ")));
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    self.reveal_title.clone(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    f3.to_string(),
                    Style::default().fg(Color::Cyan),
                )),
                Line::from("y — copy to clipboard   Esc — clear & leave"),
            ]),
            head_inner,
        );

        let secret = self
            .revealed
            .as_ref()
            .map(|s| s.expose_secret().as_str())
            .unwrap_or("");
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                secret.to_string(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )))
            .wrap(Wrap { trim: false }),
            secret_area,
        );

        let foot_inner = brand::render_faded_box(frame, footer, None);
        frame.render_widget(
            Paragraph::new(Line::from(
                "Anyone with this can spend your funds. Prefer y-copy over mouse select.",
            )),
            foot_inner,
        );
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        matches!(self.stage, Stage::Menu | Stage::HardwareHub)
            && matches!(self.device_job, DeviceJob::Idle)
    }

    /// Poll USB worker results (call each UI tick while on Keys).
    pub fn poll(&mut self, tick: u64) {
        if matches!(self.stage, Stage::DeviceBusy) {
            self.busy_tick = tick;
        }
        let job = std::mem::replace(&mut self.device_job, DeviceJob::Idle);
        match job {
            DeviceJob::Idle => {}
            DeviceJob::Preview(rx) => match rx.try_recv() {
                Ok(Ok(paths)) => {
                    self.device_paths = paths;
                    self.device_sel = 0;
                    self.stage = Stage::DevicePick;
                    self.status = if self.device_paths.is_empty() {
                        "No wallets returned".into()
                    } else {
                        "Pick path · Enter adds (no second PIN/passphrase)".into()
                    };
                }
                Ok(Err(msg)) => {
                    self.status = msg;
                    self.stage = Stage::Menu;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.device_job = DeviceJob::Preview(rx);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.status = "Trezor connect interrupted".into();
                    self.stage = Stage::Menu;
                }
            },
        }
    }

    fn start_trezor_preview(&mut self, wallet: &WalletState) {
        let ui = wallet.trezor_ui();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = vaughan_core::security::preview_trezor_live_paths_blocking(5, Some(ui))
                .map_err(|e| e.user_message());
            let _ = tx.send(result);
        });
        self.device_job = DeviceJob::Preview(rx);
        self.device_vendor = DeviceVendor::Trezor;
        self.stage = Stage::DeviceBusy;
        self.status =
            "Unlock once (PIN → passphrase if asked), then pick a path — Enter adds without re-unlock…"
                .into();
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        handle: &Handle,
        events: &EventBus,
    ) -> KeyOutcome {
        match self.stage {
            Stage::Menu => match key.code {
                KeyCode::Esc => KeyOutcome::Back,
                KeyCode::Char('1') => {
                    self.menu = MenuItem::ExportPhrase;
                    self.begin_menu_action();
                    KeyOutcome::Consumed
                }
                KeyCode::Char('2') => {
                    self.menu = MenuItem::ExportKey;
                    self.begin_menu_action();
                    KeyOutcome::Consumed
                }
                KeyCode::Char('3') => {
                    self.menu = MenuItem::ImportKey;
                    self.begin_menu_action();
                    KeyOutcome::Consumed
                }
                KeyCode::Char('4') => {
                    self.menu = MenuItem::AddLedger;
                    self.begin_menu_action();
                    KeyOutcome::Consumed
                }
                KeyCode::Char('5') => {
                    self.menu = MenuItem::AddTrezor;
                    self.begin_menu_action();
                    KeyOutcome::Consumed
                }
                KeyCode::Up | KeyCode::Down => {
                    self.menu = cycle_menu(self.menu, key.code == KeyCode::Down);
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    self.begin_menu_action();
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::NotHandled,
            },
            Stage::HardwareHub => match key.code {
                KeyCode::Esc => KeyOutcome::Back,
                KeyCode::Char('4') => {
                    self.menu = MenuItem::AddLedger;
                    self.begin_menu_action();
                    KeyOutcome::Consumed
                }
                KeyCode::Char('5') => {
                    self.menu = MenuItem::AddTrezor;
                    self.begin_menu_action();
                    KeyOutcome::Consumed
                }
                KeyCode::Up | KeyCode::Down => {
                    self.menu = cycle_hardware_hub(self.menu, key.code == KeyCode::Down);
                    KeyOutcome::Consumed
                }
                KeyCode::Enter
                    if matches!(self.menu, MenuItem::AddLedger | MenuItem::AddTrezor) =>
                {
                    self.begin_menu_action();
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    self.menu = MenuItem::AddLedger;
                    self.begin_menu_action();
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::NotHandled,
            },
            Stage::Password if matches!(self.menu, MenuItem::AddLedger | MenuItem::AddTrezor) => {
                match key.code {
                    KeyCode::Esc => {
                        self.stage = Stage::Menu;
                        KeyOutcome::Consumed
                    }
                    KeyCode::Enter => {
                        let vendor = match self.menu {
                            MenuItem::AddTrezor => DeviceVendor::Trezor,
                            _ => DeviceVendor::Ledger,
                        };
                        self.device_vendor = vendor;
                        match vendor {
                            DeviceVendor::Ledger => {
                                self.status = "Connecting to Ledger…".into();
                                match handle.block_on(wallet.preview_ledger_accounts()) {
                                    Ok(paths) => {
                                        self.device_paths = paths;
                                        self.device_sel = 0;
                                        self.stage = Stage::DevicePick;
                                        self.status = if self.device_paths.is_empty() {
                                            "No wallets returned".into()
                                        } else {
                                            "Confirm address matches the device, then Enter".into()
                                        };
                                    }
                                    Err(e) => {
                                        self.status = e.user_message();
                                        self.stage = Stage::Menu;
                                    }
                                }
                            }
                            DeviceVendor::Trezor => {
                                self.start_trezor_preview(wallet);
                            }
                        }
                        KeyOutcome::Consumed
                    }
                    _ => KeyOutcome::Consumed,
                }
            }
            Stage::DeviceBusy => match key.code {
                KeyCode::Esc => {
                    wallet.trezor_ui().request_abort();
                    self.status = "Cancelled".into();
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::Consumed,
            },
            Stage::Password => {
                if key.code == KeyCode::Esc {
                    self.clear_secret();
                    self.stage = Stage::Menu;
                    return KeyOutcome::Consumed;
                }
                match self.password.handle_key(key) {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    InputAction::Consumed => KeyOutcome::Consumed,
                    InputAction::Submitted => {
                        let pw = self.password.take_secret();
                        match self.menu {
                            MenuItem::ExportPhrase => match wallet.export_mnemonic(&pw) {
                                Ok(phrase) => {
                                    let note = match wallet.active_account_export_context() {
                                        Ok((label, _, true)) => format!(
                                            "Vault recovery phrase (HD only — F3 is imported «{label}»; use option 2 for that key)"
                                        ),
                                        Ok((label, _, false)) => {
                                            format!("Vault recovery phrase (includes HD «{label}»)")
                                        }
                                        Err(_) => "Vault recovery phrase".into(),
                                    };
                                    self.reveal_title = note;
                                    self.revealed = Some(phrase);
                                    self.stage = Stage::Reveal;
                                    self.status.clear();
                                }
                                Err(e) => self.status = e.user_message(),
                            },
                            MenuItem::ExportKey => match wallet.export_active_private_key(&pw) {
                                Ok(sk) => {
                                    let title = match wallet.active_account_export_context() {
                                        Ok((label, address, _)) => format!(
                                            "Private key · {label} · {}",
                                            short_addr(&address)
                                        ),
                                        Err(_) => "F3 wallet private key".into(),
                                    };
                                    self.reveal_title = title;
                                    self.revealed = Some(sk);
                                    self.stage = Stage::Reveal;
                                    self.status.clear();
                                }
                                Err(e) => self.status = e.user_message(),
                            },
                            MenuItem::ImportKey => match wallet.verify_password(&pw) {
                                Ok(()) => {
                                    self.verified_password = Some(pw);
                                    self.stage = Stage::ImportForm;
                                    self.import_focus = 0;
                                    self.status.clear();
                                }
                                Err(e) => self.status = e.user_message(),
                            },
                            MenuItem::AddLedger | MenuItem::AddTrezor => {
                                unreachable!("handled above")
                            }
                        }
                        KeyOutcome::Consumed
                    }
                }
            }
            Stage::DevicePick => match key.code {
                KeyCode::Esc => {
                    self.device_paths.clear();
                    self.stage = Stage::Menu;
                    KeyOutcome::Consumed
                }
                KeyCode::Up if self.device_sel > 0 => {
                    self.device_sel -= 1;
                    KeyOutcome::Consumed
                }
                KeyCode::Down if self.device_sel + 1 < self.device_paths.len() => {
                    self.device_sel += 1;
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    match self.device_vendor {
                        DeviceVendor::Ledger => {
                            if let Some((path, _)) =
                                self.device_paths.get(self.device_sel).cloned()
                            {
                                self.status = "Confirm on Ledger if asked…".into();
                                match handle.block_on(wallet.add_ledger_account(&path, "")) {
                                    Ok(account) => {
                                        self.device_paths.clear();
                                        self.stage = Stage::Menu;
                                        self.status = format!(
                                            "Added {} — F3 selected · confirm on device when signing",
                                            account.label
                                        );
                                        if let Ok(addr) = wallet.active_address() {
                                            events.publish(ProviderEvent::AccountsChanged(vec![
                                                addr.to_string(),
                                            ]));
                                        }
                                        return KeyOutcome::AccountListChanged;
                                    }
                                    Err(e) => self.status = e.user_message(),
                                }
                            }
                        }
                        DeviceVendor::Trezor => {
                            // Address already read in the preview USB session.
                            // Re-opening would EndSession + PIN + passphrase again.
                            if let Some((path, address)) =
                                self.device_paths.get(self.device_sel).cloned()
                            {
                                use vaughan_core::security::{
                                    HardwareAccountRecord, HardwareVendor, HwChainFamily,
                                };
                                let network_id =
                                    Some(wallet.networks().active().chain_id.to_string());
                                let record = HardwareAccountRecord {
                                    vendor: HardwareVendor::Trezor,
                                    family: HwChainFamily::Evm,
                                    derivation_path: path,
                                    network_id,
                                    address,
                                    label: String::new(),
                                };
                                match wallet.add_hardware_account(record) {
                                    Ok(account) => {
                                        self.device_paths.clear();
                                        self.stage = Stage::Menu;
                                        self.status = format!(
                                            "Added {} · {} — F3 selected · rename if this is a hidden wallet",
                                            account.label,
                                            short_addr(&account.address)
                                        );
                                        if let Ok(addr) = wallet.active_address() {
                                            events.publish(ProviderEvent::AccountsChanged(vec![
                                                addr.to_string(),
                                            ]));
                                        }
                                        return KeyOutcome::AccountListChanged;
                                    }
                                    Err(e) => self.status = e.user_message(),
                                }
                            }
                        }
                    }
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::Consumed,
            },
            Stage::Reveal => match key.code {
                KeyCode::Esc => {
                    self.clear_secret();
                    self.stage = Stage::Menu;
                    KeyOutcome::Consumed
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => match self.revealed.as_ref() {
                    Some(secret) => match crate::clipboard::copy_text(secret.expose_secret()) {
                        Ok(()) => {
                            let msg = if matches!(self.menu, MenuItem::ExportKey) {
                                "F3 private key copied"
                            } else {
                                "Vault recovery phrase copied"
                            };
                            KeyOutcome::Flash(msg.into())
                        }
                        Err(e) => KeyOutcome::Flash(e),
                    },
                    None => {
                        self.status = "nothing to copy".into();
                        KeyOutcome::Consumed
                    }
                },
                _ => KeyOutcome::Consumed,
            },
            Stage::ImportForm => {
                if key.code == KeyCode::Esc {
                    self.clear_secret();
                    self.stage = Stage::Menu;
                    return KeyOutcome::Consumed;
                }
                if key.code == KeyCode::Tab {
                    self.import_focus = 1 - self.import_focus;
                    return KeyOutcome::Consumed;
                }
                let action = if self.import_focus == 0 {
                    self.label.handle_key(key)
                } else {
                    self.private_key.handle_key(key)
                };
                match action {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    InputAction::Consumed => KeyOutcome::Consumed,
                    InputAction::Submitted if self.import_focus == 0 => {
                        self.import_focus = 1;
                        KeyOutcome::Consumed
                    }
                    InputAction::Submitted => {
                        let Some(pw) = self.verified_password.take() else {
                            self.stage = Stage::Password;
                            self.status = "Password required again to import.".into();
                            return KeyOutcome::Consumed;
                        };
                        let sk = self.private_key.take_secret();
                        match wallet.import_private_key(&pw, self.label.value(), &sk) {
                            Ok(account) => {
                                self.clear_secret();
                                self.status =
                                    format!("Imported {} ({})", account.label, account.address);
                                self.stage = Stage::Menu;
                            }
                            Err(e) => {
                                self.verified_password = Some(pw);
                                self.status = e.user_message();
                            }
                        }
                        KeyOutcome::Consumed
                    }
                }
            }
        }
    }
}

impl Drop for KeysView {
    fn drop(&mut self) {
        self.clear_secret();
    }
}

fn cycle_menu(menu: MenuItem, down: bool) -> MenuItem {
    use MenuItem::*;
    match (menu, down) {
        (ExportPhrase, true) => ExportKey,
        (ExportKey, true) => ImportKey,
        (ImportKey, true) => AddLedger,
        (AddLedger, true) => AddTrezor,
        (AddTrezor, true) => ExportPhrase,
        (ExportPhrase, false) => AddTrezor,
        (AddTrezor, false) => AddLedger,
        (AddLedger, false) => ImportKey,
        (ImportKey, false) => ExportKey,
        (ExportKey, false) => ExportPhrase,
    }
}

fn cycle_hardware_hub(menu: MenuItem, down: bool) -> MenuItem {
    match menu {
        MenuItem::AddLedger if down => MenuItem::AddTrezor,
        MenuItem::AddTrezor if !down => MenuItem::AddLedger,
        MenuItem::AddTrezor if down => MenuItem::AddLedger,
        MenuItem::AddLedger if !down => MenuItem::AddTrezor,
        _ => MenuItem::AddLedger,
    }
}

fn menu_line(selected: bool, text: &str) -> Line<'static> {
    let marker = if selected { ">" } else { " " };
    let style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Line::from(Span::styled(format!("{marker} {text}"), style))
}

fn short_addr(address: &str) -> String {
    let a = address.trim();
    if a.len() > 12 {
        format!("{}…{}", &a[..6], &a[a.len() - 4..])
    } else {
        a.to_string()
    }
}
