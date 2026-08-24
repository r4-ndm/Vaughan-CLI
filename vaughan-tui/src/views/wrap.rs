//! Wrap / unwrap native PLS ↔ WPLS (WETH9 `deposit` / `withdraw`).
//!
//! `e` opens. Anvil helpers live in `dex_calldata` + `browserless_anvil` tests.

use alloy::primitives::{Address, U256};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use std::str::FromStr;
use tokio::runtime::Handle;
use vaughan_core::core::{format_base_units, parse_native_amount, wpls_for_chain, WalletState};
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::dex_calldata::{build_unwrap_tx, build_wrap_tx};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Wrap,
    Unwrap,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Self::Wrap => "Wrap PLS → WPLS",
            Self::Unwrap => "Unwrap WPLS → PLS",
        }
    }

    fn toggle(self) -> Self {
        match self {
            Self::Wrap => Self::Unwrap,
            Self::Unwrap => Self::Wrap,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Input,
    Confirm,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Idle,
    Sending,
}

pub struct WrapView {
    mode: Mode,
    amount: Input,
    stage: Stage,
    busy: Busy,
    tick: u64,
    status: String,
    confirm_lines: Vec<String>,
    pending_wei: Option<U256>,
    wpls: Option<Address>,
    chain_id: u64,
}

impl WrapView {
    pub fn for_chain(chain_id: u64) -> Self {
        let wpls = wpls_for_chain(chain_id);
        let status = if wpls.is_some() {
            "←/→ Wrap↔Unwrap · amount · Enter · Esc home".into()
        } else {
            "Wrap only on PulseChain (369 / 943) — switch Net".into()
        };
        let mut amount = Input::new(false, "e.g. 1 or 0.01");
        amount.set_value("1");
        Self {
            mode: Mode::Wrap,
            amount,
            stage: Stage::Input,
            busy: Busy::Idle,
            tick: 0,
            status,
            confirm_lines: Vec::new(),
            pending_wei: None,
            wpls,
            chain_id,
        }
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::Send(Ok(hash)) => {
                self.busy = Busy::Idle;
                self.stage = Stage::Input;
                self.pending_wei = None;
                self.status = format!("{} ok ({hash})", self.mode.label());
            }
            UiJobResult::Send(Err(e)) => {
                self.busy = Busy::Idle;
                self.stage = Stage::Input;
                self.status = e.user_message();
            }
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        match self.stage {
            Stage::Confirm => {
                let inner = brand::render_faded_box(
                    frame,
                    content,
                    Some(brand::fade_line(&format!(" {} ", self.mode.label()))),
                );
                let lines: Vec<Line> = self
                    .confirm_lines
                    .iter()
                    .map(|s| Line::from(s.clone()))
                    .collect();
                frame.render_widget(Paragraph::new(lines), inner);
            }
            Stage::Input => {
                let [head, field] =
                    Layout::vertical([Constraint::Min(4), Constraint::Length(3)]).areas(content);
                let head_inner =
                    brand::render_faded_box(frame, head, Some(brand::fade_line(" Wrap / Unwrap ")));
                let wpls_s = self
                    .wpls
                    .map(|a| format!("{a:#x}"))
                    .unwrap_or_else(|| "n/a".into());
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(Span::styled(
                            self.mode.label(),
                            Style::default()
                                .fg(brand::accent_color())
                                .add_modifier(Modifier::BOLD),
                        )),
                        Line::from(format!("WPLS: {wpls_s}")),
                        Line::from("←/→ toggle mode · type amount (human units)"),
                    ]),
                    head_inner,
                );
                render_labeled_input(frame, field, "Amount", &self.amount, true);
            }
        }
        let status = if matches!(self.busy, Busy::Sending) {
            format!("{} sending…", spinner_frame(self.tick))
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
        if matches!(self.busy, Busy::Sending) {
            return match key.code {
                KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
                _ => KeyOutcome::Consumed,
            };
        }
        match self.stage {
            Stage::Confirm => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::Input;
                    self.pending_wei = None;
                    KeyOutcome::Consumed
                }
                KeyCode::Enter | KeyCode::Char('y') => self.confirm_send(wallet),
                _ => KeyOutcome::Consumed,
            },
            Stage::Input => match key.code {
                KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
                KeyCode::Left | KeyCode::Right => {
                    self.mode = self.mode.toggle();
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => self.begin_confirm(wallet),
                _ => match self.amount.handle_key(key) {
                    InputAction::Consumed => KeyOutcome::Consumed,
                    InputAction::Submitted => self.begin_confirm(wallet),
                    InputAction::Ignored => KeyOutcome::NotHandled,
                },
            },
        }
    }

    fn begin_confirm(&mut self, wallet: &WalletState) -> KeyOutcome {
        let Some(wpls) = self.wpls else {
            self.status = "Wrap only on PulseChain mainnet/testnet".into();
            return KeyOutcome::Consumed;
        };
        let raw = self.amount.value().trim();
        let wei_str = match parse_native_amount(raw, 18) {
            Ok(s) => s,
            Err(e) => {
                self.status = e.user_message();
                return KeyOutcome::Consumed;
            }
        };
        let amount = match U256::from_str(&wei_str) {
            Ok(a) if !a.is_zero() => a,
            Ok(_) => {
                self.status = "amount must be > 0".into();
                return KeyOutcome::Consumed;
            }
            Err(_) => {
                self.status = "bad amount".into();
                return KeyOutcome::Consumed;
            }
        };
        let human = format_base_units(&amount.to_string(), 18);
        let from = match wallet.active_address() {
            Ok(a) => a.to_string(),
            Err(e) => {
                self.status = e.user_message();
                return KeyOutcome::Consumed;
            }
        };
        self.confirm_lines = vec![
            self.mode.label().to_string(),
            format!("Amount: {human}"),
            format!("Contract: {wpls:#x}"),
            format!("From: {from}"),
            String::new(),
            "Enter signs · Esc cancel".into(),
        ];
        self.pending_wei = Some(amount);
        self.stage = Stage::Confirm;
        KeyOutcome::Consumed
    }

    fn confirm_send(&mut self, wallet: &WalletState) -> KeyOutcome {
        let Some(wpls) = self.wpls else {
            self.stage = Stage::Input;
            return KeyOutcome::Consumed;
        };
        let Some(amount) = self.pending_wei else {
            self.stage = Stage::Input;
            return KeyOutcome::Consumed;
        };
        let from = match wallet.active_address() {
            Ok(a) => a.to_string(),
            Err(e) => {
                self.status = e.user_message();
                self.stage = Stage::Input;
                return KeyOutcome::Consumed;
            }
        };
        let tx = match self.mode {
            Mode::Wrap => build_wrap_tx(wpls, amount, &from, self.chain_id),
            Mode::Unwrap => build_unwrap_tx(wpls, amount, &from, self.chain_id),
        };
        self.busy = Busy::Sending;
        KeyOutcome::StartJob(UiJob::SendEvm { tx })
    }
}
