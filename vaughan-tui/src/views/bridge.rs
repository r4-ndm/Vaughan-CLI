//! Bridge view — LibertySwap cross-chain (Pulse-centered), not the official Omnibridge.
//!
//! Quote → optional ERC-20 approve → broadcast on **source** chain only. Destination
//! arrival is async; v1 shows a reminder after the src tx. `f` opens this screen.

use alloy::primitives::{Address, U256};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use std::str::FromStr;
use tokio::runtime::Handle;
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::{
    format_base_units, is_whitelisted_router, parse_native_amount, BridgeChainPreset, BridgeQuote,
    WalletState, BRIDGE_CHAIN_PRESETS,
};
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::dex_calldata::build_approve_tx;
use crate::views::swap_form::{
    render_centered_amount_row, render_centered_value_field, render_form_footer, render_form_title,
    render_leg_arrow, render_text_field,
};
use crate::views::{body_areas, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmStep {
    Approve,
    Bridge,
}

impl ConfirmStep {
    fn label(self) -> &'static str {
        match self {
            Self::Approve => "approve Liberty router",
            Self::Bridge => "send LibertySwap (source chain)",
        }
    }
}

enum Stage {
    Input,
    Confirm(ConfirmStep),
    Done,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Focus {
    None,
    SrcChain,
    DstChain,
    Amount,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Idle,
    Quoting,
    Approving,
    Bridging,
}

fn preset_index(chain_id: u64) -> usize {
    BRIDGE_CHAIN_PRESETS
        .iter()
        .position(|p| p.chain_id == chain_id)
        .unwrap_or(0)
}

fn cycle_preset(idx: usize, forward: bool) -> usize {
    let n = BRIDGE_CHAIN_PRESETS.len();
    if forward {
        (idx + 1) % n
    } else {
        (idx + n - 1) % n
    }
}

pub struct BridgeView {
    stage: Stage,
    focus: Focus,
    src_idx: usize,
    dst_idx: usize,
    amount: Input,
    busy: Busy,
    tick: u64,
    status: String,
    quote: Option<BridgeQuote>,
    confirm_lines: Vec<String>,
    tx_hash: Option<String>,
    approve_hash: Option<String>,
}

impl Default for BridgeView {
    fn default() -> Self {
        Self {
            stage: Stage::Input,
            focus: Focus::None,
            src_idx: preset_index(369),
            dst_idx: preset_index(8453),
            amount: Input::new(false, "amount (e.g. 100 USDC)"),
            busy: Busy::Idle,
            tick: 0,
            status: String::new(),
            quote: None,
            confirm_lines: Vec::new(),
            tx_hash: None,
            approve_hash: None,
        }
    }
}

impl BridgeView {
    pub fn for_wallet_chain(active_chain_id: u64) -> Self {
        let mut v = Self::default();
        v.amount.set_value("100");
        if let Some(i) = BRIDGE_CHAIN_PRESETS
            .iter()
            .position(|p| p.chain_id == active_chain_id)
        {
            v.src_idx = i;
            // Prefer Base as dest when src is Pulse; Pulse when src is elsewhere.
            v.dst_idx = if active_chain_id == 369 {
                preset_index(8453)
            } else {
                preset_index(369)
            };
        }
        v.status =
            "LibertySwap · USDC cross-chain · not official Omnibridge · dest arrives async · Enter quotes"
                .into();
        v
    }

    fn src(&self) -> BridgeChainPreset {
        BRIDGE_CHAIN_PRESETS[self.src_idx]
    }

    fn dst(&self) -> BridgeChainPreset {
        BRIDGE_CHAIN_PRESETS[self.dst_idx]
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::BridgeQuote(boxed) => match *boxed {
                Ok(quote) => {
                    self.busy = Busy::Idle;
                    self.quote = Some(quote);
                    self.enter_confirm();
                }
                Err(e) => {
                    self.busy = Busy::Idle;
                    self.status = e.user_message();
                    self.stage = Stage::Input;
                }
            },
            UiJobResult::Send(Ok(receipt)) => match self.busy {
                Busy::Approving => {
                    let hash = receipt.hash;
                    self.busy = Busy::Idle;
                    self.approve_hash = Some(hash.clone());
                    self.status = format!("Approve sent ({hash}). Confirm bridge next.");
                    self.stage = Stage::Confirm(ConfirmStep::Bridge);
                    self.rebuild_confirm_lines(ConfirmStep::Bridge);
                }
                Busy::Bridging => {
                    let hash = receipt.hash;
                    self.busy = Busy::Idle;
                    self.tx_hash = Some(hash);
                    self.stage = Stage::Done;
                    self.status =
                        "Source tx broadcast. Check destination when Liberty settles (minutes)."
                            .into();
                }
                _ => {}
            },
            UiJobResult::Send(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = e.user_message();
                self.stage = Stage::Input;
            }
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState) {
        let [body, status] = body_areas(area);
        match self.stage {
            Stage::Input => self.render_input(frame, body),
            Stage::Confirm(_) => self.render_confirm(frame, body),
            Stage::Done => self.render_done(frame, body),
        }
        let status_text = match self.busy {
            Busy::Quoting => format!("{} quoting LibertySwap…", spinner_frame(self.tick)),
            Busy::Approving => format!("{} approving…", spinner_frame(self.tick)),
            Busy::Bridging => format!("{} bridging (source)…", spinner_frame(self.tick)),
            Busy::Idle => self.status.clone(),
        };
        frame.render_widget(status_paragraph(&status_text), status);
    }

    fn render_input(&self, frame: &mut Frame, area: Rect) {
        let show_quote = self.busy == Busy::Quoting;
        let constraints = vec![
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ];
        let chunks = Layout::vertical(constraints).split(area);
        let mut i = 0;

        render_form_title(frame, chunks[i], " Bridge ");
        i += 1;

        let src = self.src();
        let src_style = if self.focus == Focus::SrcChain {
            Style::default()
                .fg(brand::accent_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        };
        render_centered_value_field(
            frame,
            chunks[i],
            "From",
            Line::from(Span::styled(
                format!("{}  [{}]", src.label, src.chain_id),
                src_style,
            )),
            self.focus == Focus::SrcChain,
        );
        i += 1;

        render_leg_arrow(frame, chunks[i]);
        i += 1;

        let dst = self.dst();
        let dst_style = if self.focus == Focus::DstChain {
            Style::default()
                .fg(brand::accent_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        };
        render_centered_value_field(
            frame,
            chunks[i],
            "To",
            Line::from(Span::styled(
                format!("{}  [{}]", dst.label, dst.chain_id),
                dst_style,
            )),
            self.focus == Focus::DstChain,
        );
        i += 1;

        render_text_field(
            frame,
            chunks[i],
            "Amount",
            &self.amount,
            self.focus == Focus::Amount,
        );
        i += 1;

        if show_quote {
            render_centered_amount_row(
                frame,
                chunks[i],
                "Expected",
                Line::from(Span::styled(
                    format!("{} quoting…", spinner_frame(self.tick)),
                    Style::default().fg(Color::DarkGray),
                )),
                false,
                false,
            );
        }
        i += 1;

        render_form_footer(
            frame,
            chunks[i],
            "Tab · select · Enter · deselect · F4 quote · ↑↓ chains · wallet Net = From",
        );
    }

    fn render_done(&self, frame: &mut Frame, area: Rect) {
        let dest = self.dst().label;
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(" Bridge sent ")));
        frame.render_widget(
            Paragraph::new(format!(
                "Source broadcast done (LibertySwap).\n\
                 Approve: {}\n\
                 Bridge:  {}\n\
                 Destination ({dest}) arrives asynchronously.\n\
                 Enter again · Esc dashboard",
                self.approve_hash.as_deref().unwrap_or("—"),
                self.tx_hash.as_deref().unwrap_or("—")
            )),
            inner,
        );
    }

    fn render_confirm(&self, frame: &mut Frame, area: Rect) {
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(" Confirm ")));
        let text = self.confirm_lines.join("\n");
        frame.render_widget(
            Paragraph::new(format!("{text}\n\nEnter confirm · Esc cancel"))
                .wrap(Wrap { trim: true }),
            inner,
        );
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        match self.stage {
            Stage::Input => !matches!(self.focus, Focus::Amount),
            Stage::Confirm(_) | Stage::Done => true,
        }
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        if self.busy != Busy::Idle {
            return KeyOutcome::Consumed;
        }
        match self.stage {
            Stage::Done => match key.code {
                KeyCode::Enter => {
                    let chain_id = wallet.networks().active().chain_id;
                    *self = Self::for_wallet_chain(chain_id);
                    KeyOutcome::Consumed
                }
                KeyCode::Esc => KeyOutcome::Back,
                _ => KeyOutcome::Consumed,
            },
            Stage::Confirm(step) => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::Input;
                    self.quote = None;
                    KeyOutcome::Consumed
                }
                KeyCode::Enter | KeyCode::Char('y') => self.confirm_step(step, wallet),
                _ => KeyOutcome::Consumed,
            },
            Stage::Input => self.handle_input_key(key, wallet),
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent, wallet: &WalletState) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                if self.focus != Focus::None {
                    self.focus = Focus::None;
                    KeyOutcome::Consumed
                } else {
                    KeyOutcome::Back
                }
            }
            KeyCode::Up | KeyCode::Down
                if matches!(self.focus, Focus::SrcChain | Focus::DstChain) =>
            {
                let forward = matches!(key.code, KeyCode::Down);
                match self.focus {
                    Focus::SrcChain => {
                        self.src_idx = cycle_preset(self.src_idx, forward);
                        if self.src_idx == self.dst_idx {
                            self.dst_idx = cycle_preset(self.dst_idx, forward);
                        }
                    }
                    Focus::DstChain => {
                        self.dst_idx = cycle_preset(self.dst_idx, forward);
                        if self.dst_idx == self.src_idx {
                            self.src_idx = cycle_preset(self.src_idx, forward);
                        }
                    }
                    Focus::None | Focus::Amount => {}
                }
                self.quote = None;
                KeyOutcome::Consumed
            }
            KeyCode::Tab => {
                self.focus = self.focus_tab_forward();
                KeyOutcome::Consumed
            }
            KeyCode::BackTab => {
                self.focus = self.focus_tab_backward();
                KeyOutcome::Consumed
            }
            KeyCode::F(4) => self.start_quote(wallet),
            KeyCode::Enter if self.focus != Focus::None => {
                self.focus = Focus::None;
                KeyOutcome::Consumed
            }
            KeyCode::Enter => self.start_quote(wallet),
            _ => {
                if self.focus != Focus::Amount {
                    return KeyOutcome::Consumed;
                }
                match self.amount.handle_key(key) {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    _ => {
                        self.quote = None;
                        KeyOutcome::Consumed
                    }
                }
            }
        }
    }

    fn focus_tab_forward(&self) -> Focus {
        match self.focus {
            Focus::None => Focus::SrcChain,
            Focus::SrcChain => Focus::DstChain,
            Focus::DstChain => Focus::Amount,
            Focus::Amount => Focus::None,
        }
    }

    fn focus_tab_backward(&self) -> Focus {
        match self.focus {
            Focus::None => Focus::Amount,
            Focus::SrcChain => Focus::None,
            Focus::DstChain => Focus::SrcChain,
            Focus::Amount => Focus::DstChain,
        }
    }

    fn start_quote(&mut self, wallet: &WalletState) -> KeyOutcome {
        let active = wallet.networks().active().chain_id;
        let src = self.src();
        if active != src.chain_id {
            self.status = format!(
                "Switch Net (F1) to {} [{}] — must match From before quote",
                src.label, src.chain_id
            );
            return KeyOutcome::Consumed;
        }
        if self.src_idx == self.dst_idx {
            self.status = "From and To must differ".into();
            return KeyOutcome::Consumed;
        }
        let wei_str = match parse_native_amount(self.amount.value().trim(), 6) {
            Ok(s) => s,
            Err(e) => {
                self.status = e.user_message();
                return KeyOutcome::Consumed;
            }
        };
        let amount = match U256::from_str(&wei_str) {
            Ok(a) if !a.is_zero() => a,
            _ => {
                self.status = "amount: need non-zero USDC".into();
                return KeyOutcome::Consumed;
            }
        };
        // Liberty min ~10 USDC
        if amount < U256::from(10_000_000u64) {
            self.status = "Liberty min ≈ 10 USDC".into();
            return KeyOutcome::Consumed;
        }
        let recipient = match wallet
            .active_address()
            .ok()
            .and_then(|s| Address::from_str(s).ok())
        {
            Some(a) => a,
            None => {
                self.status = "need active wallet address".into();
                return KeyOutcome::Consumed;
            }
        };

        self.busy = Busy::Quoting;
        self.status = "quoting LibertySwap…".into();
        KeyOutcome::StartJob(UiJob::BridgeQuote {
            src_token: "USDC".into(),
            dst_token: "USDC".into(),
            amount: amount.to_string(),
            src_chain: src.chain_id,
            dst_chain: self.dst().chain_id,
            recipient: format!("{recipient:#x}"),
        })
    }

    fn enter_confirm(&mut self) {
        let Some(q) = self.quote.as_ref() else {
            return;
        };
        let step = if q.approval.is_some() {
            ConfirmStep::Approve
        } else {
            ConfirmStep::Bridge
        };
        let status = format!(
            "LibertySwap · out≈{} {} · Enter to {}",
            format_base_units(&q.dest_amount.to_string(), q.dest_token.decimals),
            q.dest_token.symbol,
            step.label()
        );
        self.rebuild_confirm_lines(step);
        self.stage = Stage::Confirm(step);
        self.status = status;
    }

    fn rebuild_confirm_lines(&mut self, step: ConfirmStep) {
        let Some(q) = self.quote.as_ref() else {
            self.confirm_lines.clear();
            return;
        };
        let in_h = format_base_units(&q.src_amount.to_string(), q.src_token.decimals);
        let out_h = format_base_units(&q.dest_amount.to_string(), q.dest_token.decimals);
        let fee_h = format_base_units(&q.fee.amount.to_string(), q.src_token.decimals);
        self.confirm_lines = vec![
            "Venue: LibertySwap (cross-chain — not official Omnibridge)".into(),
            format!("Confirm: {}", step.label()),
            format!(
                "From: {} [{}]  {} {}",
                self.src().label,
                q.src_token.chain_id,
                in_h,
                q.src_token.symbol
            ),
            format!(
                "To:   {} [{}]  ≈{} {}",
                self.dst().label,
                q.dest_token.chain_id,
                out_h,
                q.dest_token.symbol
            ),
            format!("Fee:  ≈{fee_h} ({}%)", q.fee.percentage),
            format!("Router: {:#x}", q.to),
            format!("Calldata: {} bytes", q.tx.data.len()),
            "Broadcasts on SOURCE chain only — dest arrives later.".into(),
            "Enter signs this step · Esc cancel.".into(),
        ];
    }

    fn confirm_step(&mut self, step: ConfirmStep, wallet: &WalletState) -> KeyOutcome {
        let Some(q) = self.quote.clone() else {
            self.stage = Stage::Input;
            return KeyOutcome::Consumed;
        };
        let from = match wallet.active_address() {
            Ok(a) => a.to_string(),
            Err(e) => {
                self.status = e.user_message();
                return KeyOutcome::Consumed;
            }
        };
        let chain_id = wallet.networks().active().chain_id;
        match step {
            ConfirmStep::Approve => {
                let Some(ap) = q.approval.clone() else {
                    self.stage = Stage::Confirm(ConfirmStep::Bridge);
                    self.rebuild_confirm_lines(ConfirmStep::Bridge);
                    return KeyOutcome::Consumed;
                };
                if !is_whitelisted_router(ap.spender) || ap.spender != q.tx.to {
                    self.status = format!(
                        "refusing approve: spender {:#x} not allowlisted for router {:#x}",
                        ap.spender, q.tx.to
                    );
                    self.stage = Stage::Input;
                    return KeyOutcome::Consumed;
                }
                let tx = build_approve_tx(ap.token, ap.spender, ap.amount, &from, chain_id);
                self.busy = Busy::Approving;
                KeyOutcome::StartJob(UiJob::SendEvm { tx })
            }
            ConfirmStep::Bridge => {
                if !is_whitelisted_router(q.tx.to) {
                    self.status =
                        format!("refusing bridge: router {:#x} not on allowlist", q.tx.to);
                    self.stage = Stage::Input;
                    return KeyOutcome::Consumed;
                }
                let data_hex = format!("0x{}", hex::encode(q.tx.data.as_ref()));
                let tx = EvmTransaction {
                    from,
                    to: format!("{:#x}", q.tx.to),
                    value: q.tx.value.to_string(),
                    data: Some(data_hex),
                    gas_limit: None,
                    gas_price: None,
                    max_fee_per_gas: None,
                    max_priority_fee_per_gas: None,
                    nonce: None,
                    chain_id,
                };
                self.busy = Busy::Bridging;
                KeyOutcome::StartJob(UiJob::SendEvm { tx })
            }
        }
    }
}
