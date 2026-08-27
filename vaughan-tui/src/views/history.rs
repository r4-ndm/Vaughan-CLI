//! Activity / History — session broadcasts + ERC-20 Transfer logs.
//!
//! Tab toggles **Broadcasts** (this session: cancel / speed-up pending) vs
//! **Transfers** (ERC-20 logs). `m` opens.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::chains::{TxRecord, TxStatus};
use vaughan_core::core::{
    format_base_units, mark_replaced, push_recent, BroadcastEntry, ReplaceKind, WalletState,
};
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::{body_areas, status_paragraph};

const DEFAULT_LIMIT: u32 = 40;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Broadcasts,
    Transfers,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    List,
    ConfirmReplace(ReplaceKind),
}

pub struct HistoryView {
    pane: Pane,
    stage: Stage,
    broadcasts: Vec<BroadcastEntry>,
    rows: Vec<TxRecord>,
    selected: usize,
    loading: bool,
    tick: u64,
    status: String,
    pending_replace: Option<BroadcastEntry>,
}

impl Default for HistoryView {
    fn default() -> Self {
        Self::loading()
    }
}

impl HistoryView {
    pub fn loading() -> Self {
        Self {
            pane: Pane::Broadcasts,
            stage: Stage::List,
            broadcasts: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            loading: true,
            tick: 0,
            status: String::new(),
            pending_replace: None,
        }
    }

    /// Seed with session broadcasts from the app, then load transfer logs.
    pub fn with_broadcasts(broadcasts: Vec<BroadcastEntry>) -> Self {
        let mut v = Self::loading();
        v.broadcasts = broadcasts;
        v.loading = false;
        v.refresh_status();
        v
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    fn refresh_status(&mut self) {
        self.status = match self.stage {
            Stage::ConfirmReplace(kind) => {
                format!("{} pending tx? Enter confirm · Esc back", kind.label())
            }
            Stage::List => match self.pane {
                Pane::Broadcasts => {
                    let n = self.broadcasts.len();
                    if n == 0 {
                        "No session broadcasts yet · Tab transfers · Esc home".into()
                    } else {
                        format!(
                            "{n} broadcasts · ↑/↓ · c cancel · u speed-up · r refresh · Tab transfers"
                        )
                    }
                }
                Pane::Transfers => {
                    if self.rows.is_empty() {
                        "No ERC-20 transfers in recent window · Tab broadcasts · Esc home".into()
                    } else {
                        format!(
                            "{} transfers · ↑/↓ · r reload · Tab broadcasts · Esc home",
                            self.rows.len()
                        )
                    }
                }
            },
        };
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::Activity(Ok(rows)) => {
                self.loading = false;
                self.rows = rows;
                if self.selected >= self.rows.len() && self.pane == Pane::Transfers {
                    self.selected = self.rows.len().saturating_sub(1);
                }
                self.refresh_status();
            }
            UiJobResult::Activity(Err(e)) => {
                self.loading = false;
                self.status = e.user_message();
            }
            UiJobResult::BroadcastStatuses(Ok(pairs)) => {
                for (hash, status) in pairs {
                    if let Some(e) = self.broadcasts.iter_mut().find(|b| b.hash == hash) {
                        e.status = status;
                    }
                }
                self.loading = false;
                self.refresh_status();
            }
            UiJobResult::BroadcastStatuses(Err(e)) => {
                self.loading = false;
                self.status = e.user_message();
            }
            UiJobResult::Send(Ok(receipt)) => {
                let old = receipt.entry.replaces.clone();
                push_recent(&mut self.broadcasts, receipt.entry);
                if let Some(old_hash) = old {
                    mark_replaced(&mut self.broadcasts, &old_hash, &receipt.hash);
                }
                self.stage = Stage::List;
                self.pending_replace = None;
                self.pane = Pane::Broadcasts;
                self.selected = 0;
                self.loading = false;
                self.status = format!("Replacement broadcast {}", short_hash(&receipt.hash));
            }
            UiJobResult::Send(Err(e)) => {
                self.loading = false;
                self.stage = Stage::List;
                self.pending_replace = None;
                self.status = e.user_message();
            }
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let net = wallet.networks().active();
        let pane = match self.pane {
            Pane::Broadcasts => "Broadcasts",
            Pane::Transfers => "Transfers",
        };
        let title = format!(" History · {pane} · {} ", net.name);
        let inner = brand::render_faded_box(frame, content, Some(brand::fade_line(&title)));

        if self.stage != Stage::List {
            if let Stage::ConfirmReplace(kind) = self.stage {
                let e = self.pending_replace.as_ref();
                let lines = vec![
                    Line::from(format!("{} replacement", kind.label())),
                    Line::from(""),
                    Line::from(format!(
                        "Nonce {} · {}",
                        e.map(|x| x.nonce).unwrap_or(0),
                        e.map(|x| short_hash(&x.hash)).unwrap_or_default()
                    )),
                    Line::from(format!(
                        "Same nonce, higher fees ({})",
                        match kind {
                            ReplaceKind::Cancel => "0-value self-send",
                            ReplaceKind::SpeedUp => "same payload",
                        }
                    )),
                    Line::from(""),
                    Line::from("Enter broadcast · Esc cancel"),
                ];
                frame.render_widget(Paragraph::new(lines), inner);
            }
        } else if self.loading && self.pane == Pane::Transfers {
            frame.render_widget(
                Paragraph::new(Line::from(format!("{} loading…", spinner_frame(self.tick)))),
                inner,
            );
        } else if self.pane == Pane::Broadcasts {
            self.render_broadcasts(frame, inner);
        } else if self.rows.is_empty() {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from("No token Transfer logs in the scan window."),
                    Line::from("Ag / Dex / Bridge ERC-20 moves show here."),
                    Line::from("Tab → session broadcasts (cancel / speed-up)."),
                ]),
                inner,
            );
        } else {
            self.render_transfers(frame, inner, wallet);
        }
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    fn render_broadcasts(&self, frame: &mut Frame, inner: Rect) {
        if self.broadcasts.is_empty() {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from("No broadcasts this session."),
                    Line::from("Sends from Send / Ag / Dex / Browser land here."),
                    Line::from("Pending rows: c = cancel · u = speed-up."),
                ]),
                inner,
            );
            return;
        }
        let items: Vec<ListItem> = self
            .broadcasts
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let st = match b.status {
                    TxStatus::Pending => "pend",
                    TxStatus::Confirmed => "ok  ",
                    TxStatus::Failed => "fail",
                };
                let line = format!(
                    "{st}  nonce {:>4}  {:<10}  {}",
                    b.nonce,
                    b.label,
                    short_hash(&b.hash)
                );
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

    fn render_transfers(&self, frame: &mut Frame, inner: Rect, wallet: &WalletState) {
        let me = wallet.active_address().ok().map(|a| a.to_string());
        let items: Vec<ListItem> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let sym = r.token_symbol.as_deref().unwrap_or("TOKEN");
                let decimals = 18u8;
                let amt = format_base_units(&r.value, decimals);
                let dir = match &me {
                    Some(m) if r.to.eq_ignore_ascii_case(m) => "IN ",
                    Some(m) if r.from.eq_ignore_ascii_case(m) => "OUT",
                    _ => "···",
                };
                let blk = r
                    .block_number
                    .map(|b| format!("#{b}"))
                    .unwrap_or_else(|| "?".into());
                let line = format!("{dir} {amt} {sym}  {blk}  {}", short_hash(&r.hash));
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

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        _wallet: &mut WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        if self.loading && self.pane == Pane::Transfers && self.stage == Stage::List {
            return match key.code {
                KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
                _ => KeyOutcome::Consumed,
            };
        }

        if let Stage::ConfirmReplace(kind) = self.stage {
            return match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    self.pending_replace = None;
                    self.refresh_status();
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    let Some(entry) = self.pending_replace.clone() else {
                        self.stage = Stage::List;
                        return KeyOutcome::Consumed;
                    };
                    self.loading = true;
                    self.status = format!("{}ing…", kind.label());
                    KeyOutcome::StartJob(UiJob::ReplaceBroadcast { entry, kind })
                }
                _ => KeyOutcome::Consumed,
            };
        }

        match key.code {
            KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
            KeyCode::Tab => {
                self.pane = match self.pane {
                    Pane::Broadcasts => Pane::Transfers,
                    Pane::Transfers => Pane::Broadcasts,
                };
                self.selected = 0;
                self.refresh_status();
                if self.pane == Pane::Transfers && self.rows.is_empty() {
                    self.loading = true;
                    return KeyOutcome::StartJob(UiJob::RefreshActivity {
                        limit: DEFAULT_LIMIT,
                    });
                }
                KeyOutcome::Consumed
            }
            KeyCode::Char('r') | KeyCode::Char('R') => match self.pane {
                Pane::Transfers => {
                    self.loading = true;
                    self.status = "reloading…".into();
                    KeyOutcome::StartJob(UiJob::RefreshActivity {
                        limit: DEFAULT_LIMIT,
                    })
                }
                Pane::Broadcasts => {
                    if self.broadcasts.is_empty() {
                        self.refresh_status();
                        return KeyOutcome::Consumed;
                    }
                    self.loading = true;
                    self.status = "refreshing statuses…".into();
                    KeyOutcome::StartJob(UiJob::RefreshBroadcastStatuses {
                        hashes: self.broadcasts.iter().map(|b| b.hash.clone()).collect(),
                    })
                }
            },
            KeyCode::Char('c') | KeyCode::Char('C') if self.pane == Pane::Broadcasts => {
                self.begin_replace(ReplaceKind::Cancel)
            }
            KeyCode::Char('u') | KeyCode::Char('U') if self.pane == Pane::Broadcasts => {
                self.begin_replace(ReplaceKind::SpeedUp)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                KeyOutcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let len = match self.pane {
                    Pane::Broadcasts => self.broadcasts.len(),
                    Pane::Transfers => self.rows.len(),
                };
                if len > 0 {
                    self.selected = (self.selected + 1).min(len - 1);
                }
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::NotHandled,
        }
    }

    fn begin_replace(&mut self, kind: ReplaceKind) -> KeyOutcome {
        let Some(entry) = self.broadcasts.get(self.selected).cloned() else {
            self.status = "nothing selected".into();
            return KeyOutcome::Consumed;
        };
        if !entry.is_replaceable() {
            self.status = "only pending txs can be cancelled / sped up".into();
            return KeyOutcome::Consumed;
        }
        if entry.max_fee_per_gas.is_none() || entry.max_priority_fee_per_gas.is_none() {
            self.status = "cannot replace: missing EIP-1559 fees on record".into();
            return KeyOutcome::Consumed;
        }
        self.pending_replace = Some(entry);
        self.stage = Stage::ConfirmReplace(kind);
        self.refresh_status();
        KeyOutcome::Consumed
    }

    pub fn initial_job(&self) -> UiJob {
        if self.broadcasts.is_empty() {
            UiJob::RefreshActivity {
                limit: DEFAULT_LIMIT,
            }
        } else {
            UiJob::RefreshBroadcastStatuses {
                hashes: self.broadcasts.iter().map(|b| b.hash.clone()).collect(),
            }
        }
    }
}

fn short_hash(hash: &str) -> String {
    if hash.len() > 12 {
        format!("{}…{}", &hash[..8], &hash[hash.len() - 4..])
    } else {
        hash.to_string()
    }
}
