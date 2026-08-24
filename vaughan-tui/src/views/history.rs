//! Activity / History — recent ERC-20 Transfer logs for the active account.
//!
//! Native-only sends without a token log are not indexed (no explorer). `m` opens.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::chains::TxRecord;
use vaughan_core::core::{format_base_units, WalletState};
use vaughan_core::error::WalletError;
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::{body_areas, status_paragraph};

const DEFAULT_LIMIT: u32 = 40;

pub struct HistoryView {
    rows: Vec<TxRecord>,
    selected: usize,
    loading: bool,
    tick: u64,
    status: String,
}

impl Default for HistoryView {
    fn default() -> Self {
        Self::loading()
    }
}

impl HistoryView {
    pub fn loading() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            loading: true,
            tick: 0,
            status: String::new(),
        }
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::Activity(Ok(rows)) => {
                self.loading = false;
                self.rows = rows;
                if self.selected >= self.rows.len() {
                    self.selected = self.rows.len().saturating_sub(1);
                }
                self.status = if self.rows.is_empty() {
                    "No ERC-20 transfers in recent window · Esc home".into()
                } else {
                    format!("{} transfers · ↑/↓ · r reload · Esc home", self.rows.len())
                };
            }
            UiJobResult::Activity(Err(e)) => {
                self.loading = false;
                self.status = e.user_message();
            }
            _ => {}
        }
    }

    pub fn apply_activity(&mut self, result: Result<Vec<TxRecord>, WalletError>) {
        self.apply_job_result(UiJobResult::Activity(result));
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let net = wallet.networks().active();
        let title = format!(" History · {} ", net.name);
        let inner = brand::render_faded_box(frame, content, Some(brand::fade_line(&title)));

        if self.loading {
            frame.render_widget(
                Paragraph::new(Line::from(format!("{} loading…", spinner_frame(self.tick)))),
                inner,
            );
        } else if self.rows.is_empty() {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from("No token Transfer logs in the scan window."),
                    Line::from("Ag / Dex / Bridge ERC-20 moves show here."),
                ]),
                inner,
            );
        } else {
            let me = wallet.active_address().ok().map(|a| a.to_string());
            let items: Vec<ListItem> = self
                .rows
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let sym = r.token_symbol.as_deref().unwrap_or("TOKEN");
                    let decimals = 18u8; // display best-effort; raw still shown short
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
                    let hash_short = if r.hash.len() > 12 {
                        format!("{}…{}", &r.hash[..8], &r.hash[r.hash.len() - 4..])
                    } else {
                        r.hash.clone()
                    };
                    let line = format!("{dir} {amt} {sym}  {blk}  {hash_short}");
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
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        _wallet: &mut WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        if self.loading {
            return match key.code {
                KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
                _ => KeyOutcome::Consumed,
            };
        }
        match key.code {
            KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.loading = true;
                self.status = "reloading…".into();
                KeyOutcome::StartJob(UiJob::RefreshActivity {
                    limit: DEFAULT_LIMIT,
                })
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
            _ => KeyOutcome::NotHandled,
        }
    }

    pub fn initial_job() -> UiJob {
        UiJob::RefreshActivity {
            limit: DEFAULT_LIMIT,
        }
    }
}
