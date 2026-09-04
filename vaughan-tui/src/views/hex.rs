//! HEX stake manager — list pHEX stakes; start / end with confirm.
//!
//! `u` opens. PulseChain mainnet (369) only. Hearts use 8 decimals.

use alloy::primitives::U256;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph},
    Frame,
};
use std::str::FromStr;
use tokio::runtime::Handle;
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::{
    encode_stake_end, encode_stake_start, format_display_amount, parse_native_amount, phex_address,
    HexGlobalState, HexStakeResult, HexStakeRow, HexStakesForAddress, WalletState, MAX_STAKE_DAYS,
    MIN_STAKE_DAYS, PHEX_HEARTS_DECIMALS,
};
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    List,
    StartInput,
    StartConfirm,
    EndConfirm,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Idle,
    Loading,
    Sending,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StartFocus {
    Amount,
    Days,
}

pub struct HexView {
    stakes: Vec<HexStakeRow>,
    stake_count: String,
    globals: Option<HexGlobalState>,
    selected: usize,
    stage: Stage,
    busy: Busy,
    tick: u64,
    status: String,
    confirm_lines: Vec<String>,
    pending_tx: Option<EvmTransaction>,
    amount: Input,
    days: Input,
    start_focus: StartFocus,
    chain_id: u64,
    owner: String,
    list_gen: u64,
}

impl HexView {
    pub fn for_chain(chain_id: u64, owner: &str) -> Self {
        let ok = chain_id == 369;
        Self {
            stakes: Vec::new(),
            stake_count: "0".into(),
            globals: None,
            selected: 0,
            stage: Stage::List,
            busy: if ok { Busy::Loading } else { Busy::Idle },
            tick: 0,
            status: if ok {
                "loading HEX stakes…".into()
            } else {
                "HEX stakes on PulseChain mainnet (369) — switch Net".into()
            },
            confirm_lines: Vec::new(),
            pending_tx: None,
            amount: {
                let mut i = Input::new(false, "e.g. 1000");
                i.set_value("1000");
                i
            },
            days: {
                let mut i = Input::new(false, "1–5555");
                i.set_value("365");
                i
            },
            start_focus: StartFocus::Amount,
            chain_id,
            owner: owner.to_string(),
            list_gen: 0,
        }
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn initial_job(&self) -> Option<UiJob> {
        if self.chain_id != 369 || self.owner.is_empty() {
            return None;
        }
        Some(UiJob::RefreshHexStakes {
            owner: self.owner.clone(),
            gen: self.list_gen,
        })
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::HexStakes {
                gen,
                owner,
                stakes,
                globals,
            } => {
                if gen != self.list_gen || !owner.eq_ignore_ascii_case(&self.owner) {
                    return;
                }
                self.busy = Busy::Idle;
                match stakes {
                    HexStakeResult::Ok { data, .. } => {
                        self.apply_stakes(data);
                    }
                    HexStakeResult::Err(f) => {
                        self.stakes.clear();
                        self.status = f.reason;
                    }
                }
                if let HexStakeResult::Ok { data, .. } = globals {
                    let day = data.current_day.clone();
                    self.globals = Some(data);
                    if self.stakes.is_empty() {
                        self.status =
                            format!("No stakes · day {day} · s start · r reload · Esc home");
                    }
                }
            }
            UiJobResult::Send(Ok(receipt)) => {
                let hash = receipt.hash;
                self.busy = Busy::Idle;
                self.stage = Stage::List;
                self.pending_tx = None;
                self.status = format!("HEX tx sent ({hash}). Reloading…");
                self.list_gen = self.list_gen.wrapping_add(1);
                self.busy = Busy::Loading;
            }
            UiJobResult::Send(Err(e)) => {
                self.busy = Busy::Idle;
                self.stage = Stage::List;
                self.pending_tx = None;
                self.status = e.user_message();
            }
            _ => {}
        }
    }

    /// Follow-up reload after a successful stake tx.
    pub fn reload_job(&self) -> Option<UiJob> {
        if matches!(self.busy, Busy::Loading) && self.status.contains("Reloading") {
            Some(UiJob::RefreshHexStakes {
                owner: self.owner.clone(),
                gen: self.list_gen,
            })
        } else {
            None
        }
    }

    fn apply_stakes(&mut self, data: HexStakesForAddress) {
        self.stake_count = data.stake_count;
        self.stakes = data.stakes;
        if self.selected >= self.stakes.len() {
            self.selected = self.stakes.len().saturating_sub(1);
        }
        let day = self
            .globals
            .as_ref()
            .map(|g| g.current_day.as_str())
            .unwrap_or("?");
        self.status = if self.stakes.is_empty() {
            format!("No stakes · day {day} · s start · r reload · Esc home")
        } else {
            format!(
                "{} stake(s) · day {day} · Enter end · s start · r reload",
                self.stakes.len()
            )
        };
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        match self.stage {
            Stage::List => self.render_list(frame, content),
            Stage::StartInput => self.render_start_input(frame, content),
            Stage::StartConfirm | Stage::EndConfirm => {
                let title = if matches!(self.stage, Stage::StartConfirm) {
                    " Confirm stakeStart "
                } else {
                    " Confirm stakeEnd "
                };
                let inner = brand::render_faded_box(frame, content, Some(brand::fade_line(title)));
                let lines: Vec<Line> = self
                    .confirm_lines
                    .iter()
                    .map(|s| Line::from(s.clone()))
                    .collect();
                frame.render_widget(Paragraph::new(lines), inner);
            }
        }
        let status = match self.busy {
            Busy::Loading => format!("{} loading…", spinner_frame(self.tick)),
            Busy::Sending => format!("{} sending…", spinner_frame(self.tick)),
            Busy::Idle => self.status.clone(),
        };
        frame.render_widget(status_paragraph(&status), status_area);
    }

    fn render_list(&self, frame: &mut Frame, area: Rect) {
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(" HEX stakes ")));
        let phex = format!("{:#x}", phex_address());
        let mut lines: Vec<ListItem> = vec![ListItem::new(Line::from(format!("pHEX {phex}")))];
        if let Some(g) = &self.globals {
            lines.push(ListItem::new(Line::from(format!(
                "day {} · shareRate {} · lockedHearts {}",
                g.current_day, g.share_rate, g.locked_hearts_total
            ))));
        }
        if self.stakes.is_empty() {
            lines.push(ListItem::new(Line::from("— no stakes —")));
        } else {
            for (i, s) in self.stakes.iter().enumerate() {
                let hearts = format_display_amount(&s.staked_hearts, PHEX_HEARTS_DECIMALS, 4);
                let lock = if s.still_locked {
                    "locked"
                } else {
                    "ended"
                };
                let mark = if i == self.selected { "›" } else { " " };
                lines.push(ListItem::new(Line::from(format!(
                    "{mark} #{i} id={} · {hearts} HEX · {}d · {lock}",
                    s.stake_id, s.staked_days
                ))));
            }
        }
        frame.render_widget(List::new(lines), inner);
    }

    fn render_start_input(&self, frame: &mut Frame, area: Rect) {
        let [head, amt, days] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .areas(area);
        let head_inner =
            brand::render_faded_box(frame, head, Some(brand::fade_line(" Start HEX stake ")));
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "Stake pHEX (8 decimals) for N HEX days",
                    Style::default()
                        .fg(brand::accent_color())
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(format!(
                    "Tab fields · Enter confirm · days {MIN_STAKE_DAYS}–{MAX_STAKE_DAYS}"
                )),
            ]),
            head_inner,
        );
        render_labeled_input(
            frame,
            amt,
            "Amount (HEX)",
            &self.amount,
            matches!(self.start_focus, StartFocus::Amount),
        );
        render_labeled_input(
            frame,
            days,
            "Days",
            &self.days,
            matches!(self.start_focus, StartFocus::Days),
        );
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        // Never defer chip keys: List needs `s`/`r`; confirm must not navigate away.
        false
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        if matches!(self.busy, Busy::Loading | Busy::Sending) {
            return KeyOutcome::Consumed;
        }
        match self.stage {
            Stage::List => self.handle_list(key, wallet),
            Stage::StartInput => self.handle_start_input(key, wallet),
            Stage::StartConfirm | Stage::EndConfirm => self.handle_confirm(key),
        }
    }

    fn handle_list(&mut self, key: KeyEvent, wallet: &WalletState) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => KeyOutcome::Back,
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if self.chain_id != 369 {
                    return KeyOutcome::Consumed;
                }
                self.owner = wallet
                    .active_address()
                    .ok()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| self.owner.clone());
                self.list_gen = self.list_gen.wrapping_add(1);
                self.busy = Busy::Loading;
                self.status = "reloading…".into();
                KeyOutcome::StartJob(UiJob::RefreshHexStakes {
                    owner: self.owner.clone(),
                    gen: self.list_gen,
                })
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                if self.chain_id != 369 {
                    self.status = "HEX stakes only on PulseChain 369".into();
                    return KeyOutcome::Consumed;
                }
                self.stage = Stage::StartInput;
                self.start_focus = StartFocus::Amount;
                self.status = "amount · days · Enter · Esc cancel".into();
                KeyOutcome::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if !self.stakes.is_empty() {
                    self.selected = self.selected.saturating_sub(1);
                }
                KeyOutcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.stakes.is_empty() {
                    self.selected = (self.selected + 1).min(self.stakes.len() - 1);
                }
                KeyOutcome::Consumed
            }
            KeyCode::Enter => {
                let from = match wallet.active_address() {
                    Ok(a) => a,
                    Err(e) => {
                        self.status = e.user_message();
                        return KeyOutcome::Consumed;
                    }
                };
                let Some(row) = self.stakes.get(self.selected).cloned() else {
                    self.status = "No stake selected — press s to start".into();
                    return KeyOutcome::Consumed;
                };
                match self.build_end_tx(&from, &row) {
                    Ok(tx) => {
                        let hearts =
                            format_display_amount(&row.staked_hearts, PHEX_HEARTS_DECIMALS, 4);
                        self.confirm_lines = vec![
                            format!(
                                "End stake #{index} id={id}",
                                index = row.index,
                                id = row.stake_id
                            ),
                            format!("Hearts:  {hearts} HEX"),
                            format!("Days:    {}", row.staked_days),
                            format!(
                                "Status:  {}",
                                if row.still_locked {
                                    "still locked — early end = penalty"
                                } else {
                                    "unlockedDay set"
                                }
                            ),
                            "Enter/y confirm · Esc cancel".into(),
                        ];
                        self.pending_tx = Some(tx);
                        self.stage = Stage::EndConfirm;
                        self.status = "confirm end stake".into();
                    }
                    Err(e) => self.status = e,
                }
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::NotHandled,
        }
    }

    fn handle_start_input(&mut self, key: KeyEvent, wallet: &WalletState) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                self.stage = Stage::List;
                self.status = "cancelled".into();
                KeyOutcome::Consumed
            }
            KeyCode::Tab => {
                self.start_focus = match self.start_focus {
                    StartFocus::Amount => StartFocus::Days,
                    StartFocus::Days => StartFocus::Amount,
                };
                KeyOutcome::Consumed
            }
            KeyCode::Enter => {
                let from = match wallet.active_address() {
                    Ok(a) => a,
                    Err(e) => {
                        self.status = e.user_message();
                        return KeyOutcome::Consumed;
                    }
                };
                match self.prepare_start_confirm(&from) {
                    Ok(()) => KeyOutcome::Consumed,
                    Err(e) => {
                        self.status = e;
                        KeyOutcome::Consumed
                    }
                }
            }
            _ => {
                let input = match self.start_focus {
                    StartFocus::Amount => &mut self.amount,
                    StartFocus::Days => &mut self.days,
                };
                match input.handle_key(key) {
                    InputAction::Consumed => KeyOutcome::Consumed,
                    InputAction::Submitted => self.handle_start_input(
                        KeyEvent::new(KeyCode::Enter, key.modifiers),
                        wallet,
                    ),
                    InputAction::Ignored => KeyOutcome::NotHandled,
                }
            }
        }
    }

    fn prepare_start_confirm(&mut self, from: &str) -> Result<(), String> {
        let hearts_raw = parse_native_amount(self.amount.value(), PHEX_HEARTS_DECIMALS)
            .map_err(|e| e.user_message())?;
        let hearts = U256::from_str(&hearts_raw).map_err(|e| e.to_string())?;
        let days: u64 = self
            .days
            .value()
            .trim()
            .parse()
            .map_err(|_| "days must be an integer".to_string())?;
        let tx = self.build_start_tx(from, hearts, days)?;
        let human = format_display_amount(&hearts.to_string(), PHEX_HEARTS_DECIMALS, 8);
        self.confirm_lines = vec![
            "Start HEX stake".into(),
            format!("Amount:  {human} HEX"),
            format!("Days:    {days}"),
            format!("Target:  {:#x}", phex_address()),
            "Early endStake incurs a penalty.".into(),
            "Enter/y confirm · Esc cancel".into(),
        ];
        self.pending_tx = Some(tx);
        self.stage = Stage::StartConfirm;
        self.status = "confirm stakeStart".into();
        Ok(())
    }

    fn handle_confirm(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                self.pending_tx = None;
                self.stage = if matches!(self.stage, Stage::StartConfirm) {
                    Stage::StartInput
                } else {
                    Stage::List
                };
                self.status = "cancelled".into();
                KeyOutcome::Consumed
            }
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                let Some(tx) = self.pending_tx.take() else {
                    self.status = "nothing to send".into();
                    return KeyOutcome::Consumed;
                };
                self.busy = Busy::Sending;
                self.status = "sending…".into();
                KeyOutcome::StartJob(UiJob::SendEvm { tx })
            }
            // Swallow footer chips on confirm — Esc to leave.
            _ => KeyOutcome::Consumed,
        }
    }

    fn build_start_tx(
        &self,
        from: &str,
        hearts: U256,
        days: u64,
    ) -> Result<EvmTransaction, String> {
        let data = encode_stake_start(hearts, days).map_err(|e| e.user_message())?;
        Ok(EvmTransaction {
            from: from.to_string(),
            to: format!("{:#x}", phex_address()),
            value: "0".into(),
            data: Some(format!("0x{}", hex::encode(data.as_ref()))),
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id: self.chain_id,
        })
    }

    fn build_end_tx(&self, from: &str, row: &HexStakeRow) -> Result<EvmTransaction, String> {
        let stake_id: u64 = row
            .stake_id
            .parse()
            .map_err(|_| "bad stake_id".to_string())?;
        let data = encode_stake_end(u64::from(row.index), stake_id).map_err(|e| e.user_message())?;
        Ok(EvmTransaction {
            from: from.to_string(),
            to: format!("{:#x}", phex_address()),
            value: "0".into(),
            data: Some(format!("0x{}", hex::encode(data.as_ref()))),
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id: self.chain_id,
        })
    }
}

/// Shared loader for [`UiJob::RefreshHexStakes`].
pub async fn load_hex_stakes(
    rpc_url: &str,
    owner: &str,
) -> (
    HexStakeResult<HexStakesForAddress>,
    HexStakeResult<HexGlobalState>,
) {
    let stakes =
        vaughan_core::core::fetch_hex_stakes_for_address(rpc_url, owner, "phex", 50).await;
    let globals = vaughan_core::core::fetch_hex_global_state(rpc_url, "phex").await;
    (stakes, globals)
}
