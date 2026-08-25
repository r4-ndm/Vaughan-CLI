//! Approvals manager — list non-zero ERC-20 allowances vs known spenders; revoke.
//!
//! `j` opens. Enter confirms `approve(spender, 0)`.

use alloy::primitives::Address;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph},
    Frame,
};
use std::str::FromStr;
use tokio::runtime::Handle;
use vaughan_core::chains::AllowanceEntry;
use vaughan_core::core::{format_base_units, WalletState};
use vaughan_core::error::WalletError;
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::dex_calldata::build_revoke_tx;
use crate::views::{body_areas, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    List,
    Confirm,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Idle,
    Scanning,
    Revoking,
}

pub struct ApprovalsView {
    rows: Vec<AllowanceEntry>,
    selected: usize,
    stage: Stage,
    busy: Busy,
    tick: u64,
    status: String,
    confirm_lines: Vec<String>,
}

impl Default for ApprovalsView {
    fn default() -> Self {
        Self::loading()
    }
}

impl ApprovalsView {
    pub fn loading() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            stage: Stage::List,
            busy: Busy::Scanning,
            tick: 0,
            status: "scanning allowances…".into(),
            confirm_lines: Vec::new(),
        }
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::Allowances(Ok(rows)) => {
                self.busy = Busy::Idle;
                self.rows = rows;
                if self.selected >= self.rows.len() {
                    self.selected = self.rows.len().saturating_sub(1);
                }
                self.status = if self.rows.is_empty() {
                    "No open allowances for known Ag/Dex/Bridge spenders · Esc home".into()
                } else {
                    format!(
                        "{} allowance(s) · Enter revoke · r reload · Esc home",
                        self.rows.len()
                    )
                };
            }
            UiJobResult::Allowances(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = e.user_message();
            }
            UiJobResult::Send(Ok(receipt)) => {
                let hash = receipt.hash;
                self.busy = Busy::Idle;
                self.stage = Stage::List;
                self.status = format!("Revoke sent ({hash}). Reloading…");
                self.busy = Busy::Scanning;
            }
            UiJobResult::Send(Err(e)) => {
                self.busy = Busy::Idle;
                self.stage = Stage::List;
                self.status = e.user_message();
            }
            _ => {}
        }
    }

    pub fn apply_allowances(&mut self, result: Result<Vec<AllowanceEntry>, WalletError>) {
        self.apply_job_result(UiJobResult::Allowances(result));
    }

    pub fn needs_reload_after_send(&self) -> bool {
        matches!(self.busy, Busy::Scanning) && self.status.contains("Reloading")
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        match self.stage {
            Stage::Confirm => {
                let inner = brand::render_faded_box(
                    frame,
                    content,
                    Some(brand::fade_line(" Revoke allowance ")),
                );
                let lines: Vec<Line> = self
                    .confirm_lines
                    .iter()
                    .map(|s| Line::from(s.clone()))
                    .collect();
                frame.render_widget(Paragraph::new(lines), inner);
            }
            Stage::List => {
                let inner =
                    brand::render_faded_box(frame, content, Some(brand::fade_line(" Approvals ")));
                if matches!(self.busy, Busy::Scanning) {
                    frame.render_widget(
                        Paragraph::new(Line::from(format!(
                            "{} scanning…",
                            spinner_frame(self.tick)
                        ))),
                        inner,
                    );
                } else if self.rows.is_empty() {
                    frame.render_widget(
                        Paragraph::new("No non-zero allowances against known routers."),
                        inner,
                    );
                } else {
                    let items: Vec<ListItem> = self
                        .rows
                        .iter()
                        .enumerate()
                        .map(|(i, r)| {
                            let amt = format_base_units(&r.amount, r.token_decimals);
                            let sp = if r.spender.len() > 12 {
                                format!("{}…{}", &r.spender[..8], &r.spender[r.spender.len() - 4..])
                            } else {
                                r.spender.clone()
                            };
                            let line =
                                format!("{} → {} {} ({amt})", r.token_symbol, r.spender_label, sp);
                            let style = if i == self.selected {
                                Style::default()
                                    .fg(brand::accent_color())
                                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                            } else {
                                Style::default().fg(brand::body_color())
                            };
                            ListItem::new(Line::from(Span::styled(line, style)))
                        })
                        .collect();
                    frame.render_widget(List::new(items), inner);
                }
            }
        }
        let status = if matches!(self.busy, Busy::Revoking) {
            format!("{} broadcasting revoke…", spinner_frame(self.tick))
        } else {
            self.status.clone()
        };
        frame.render_widget(status_paragraph(&status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        if matches!(self.busy, Busy::Scanning | Busy::Revoking) {
            return match key.code {
                KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
                _ => KeyOutcome::Consumed,
            };
        }
        match self.stage {
            Stage::Confirm => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    KeyOutcome::Consumed
                }
                KeyCode::Enter | KeyCode::Char('y') => self.confirm_revoke(wallet),
                _ => KeyOutcome::Consumed,
            },
            Stage::List => match key.code {
                KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    self.busy = Busy::Scanning;
                    self.status = "scanning…".into();
                    KeyOutcome::StartJob(UiJob::RefreshAllowances)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.selected = self.selected.saturating_sub(1);
                    KeyOutcome::Consumed
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if !self.rows.is_empty() {
                        self.selected = (self.selected + 1).min(self.rows.len() - 1);
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    if let Some(row) = self.rows.get(self.selected) {
                        let amt = format_base_units(&row.amount, row.token_decimals);
                        self.confirm_lines = vec![
                            format!("Token:   {} ({})", row.token_symbol, row.token),
                            format!("Spender: {} ({})", row.spender_label, row.spender),
                            format!("Amount:  {amt}"),
                            String::new(),
                            "Enter → approve(spender, 0) · Esc cancel".into(),
                        ];
                        self.stage = Stage::Confirm;
                    }
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::NotHandled,
            },
        }
    }

    fn confirm_revoke(&mut self, wallet: &WalletState) -> KeyOutcome {
        let Some(row) = self.rows.get(self.selected).cloned() else {
            self.stage = Stage::List;
            return KeyOutcome::Consumed;
        };
        let from = match wallet.active_address() {
            Ok(a) => a.to_string(),
            Err(e) => {
                self.status = e.user_message();
                self.stage = Stage::List;
                return KeyOutcome::Consumed;
            }
        };
        let Ok(token) = Address::from_str(&row.token) else {
            self.status = "bad token address".into();
            self.stage = Stage::List;
            return KeyOutcome::Consumed;
        };
        let Ok(spender) = Address::from_str(&row.spender) else {
            self.status = "bad spender".into();
            self.stage = Stage::List;
            return KeyOutcome::Consumed;
        };
        let chain_id = wallet.networks().active().chain_id;
        let tx = build_revoke_tx(token, spender, &from, chain_id);
        self.busy = Busy::Revoking;
        KeyOutcome::StartJob(UiJob::SendEvm { tx })
    }

    pub fn initial_job() -> UiJob {
        UiJob::RefreshAllowances
    }

    /// After a successful revoke broadcast, queue a rescan.
    pub fn reload_job(&self) -> Option<UiJob> {
        if self.needs_reload_after_send() {
            Some(UiJob::RefreshAllowances)
        } else {
            None
        }
    }
}
