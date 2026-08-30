//! V3 concentrated liquidity view (wiz4rd NPM on Pulse testnet 943).
//!
//! Browserless LP: list positions, mint, and collect fees via [`dex_lp`].

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::core::wiz4rd::WZRD_SMOKE_943;
use vaughan_core::core::{
    build_v3_collect_evm, build_v3_mint_evm, default_full_range_ticks, min_out_after_slippage,
    wpls_for_chain, V3PositionInfo, WalletState, DEFAULT_DEX_SLIPPAGE_BPS,
};
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::brand;
use crate::input::Input;
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::{body_areas, parse_swap_amount, parse_token_address, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    List,
    Mint,
    Collect,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::Mint => "Mint",
            Self::Collect => "Collect",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::List => Self::Mint,
            Self::Mint => Self::Collect,
            Self::Collect => Self::List,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::List => Self::Collect,
            Self::Mint => Self::List,
            Self::Collect => Self::Mint,
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
    Loading,
    Sending,
}

pub struct LpView {
    tab: Tab,
    stage: Stage,
    busy: Busy,
    tick: u64,
    status: String,
    chain_id: u64,
    positions: Vec<V3PositionInfo>,
    sel: usize,
    /// Mint form
    token0: Input,
    token1: Input,
    fee: Input,
    amount0: Input,
    amount1: Input,
    confirm_lines: Vec<Line<'static>>,
    pending_tx: Option<vaughan_core::chains::EvmTransaction>,
}

impl LpView {
    pub fn for_chain(chain_id: u64) -> Self {
        let mut token0 = Input::new(false, "token0 address");
        let mut token1 = Input::new(false, "token1 address");
        let mut fee = Input::new(false, "500");
        fee.set_value("500");
        let mut amount0 = Input::new(false, "amount0");
        amount0.set_value("1");
        let mut amount1 = Input::new(false, "amount1");
        amount1.set_value("1");
        if chain_id == 943 {
            // Uni V3 pool key requires token0 < token1 (WZRD < tWPLS on 943).
            token0.set_value(WZRD_SMOKE_943);
            if let Some(wpls) = wpls_for_chain(chain_id) {
                token1.set_value(format!("{wpls}"));
            }
        }
        let status = if chain_id == 943 {
            "←/→ tab · r reload · Enter act · Esc back".into()
        } else {
            "V3 LP wired on Pulse testnet (943) — F1 Net".into()
        };
        Self {
            tab: Tab::List,
            stage: Stage::Input,
            busy: Busy::Idle,
            tick: 0,
            status,
            chain_id,
            positions: Vec::new(),
            sel: 0,
            token0,
            token1,
            fee,
            amount0,
            amount1,
            confirm_lines: Vec::new(),
            pending_tx: None,
        }
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        self.stage == Stage::Input && self.busy == Busy::Idle
    }

    pub fn initial_job(&self, wallet: &WalletState) -> Option<UiJob> {
        self.list_job(wallet)
    }

    fn list_job(&self, wallet: &WalletState) -> Option<UiJob> {
        if self.chain_id != 943 {
            return None;
        }
        let owner = wallet.active_address().ok()?.to_string();
        let rpc = wallet.active_rpc_url();
        Some(UiJob::LpListPositions {
            chain_id: self.chain_id,
            rpc_url: rpc,
            owner,
        })
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::LpPositions(Ok(rows)) => {
                self.busy = Busy::Idle;
                self.positions = rows;
                if self.sel >= self.positions.len() && !self.positions.is_empty() {
                    self.sel = 0;
                }
                self.status = format!("{} position(s)", self.positions.len());
            }
            UiJobResult::LpPositions(Err(e)) => {
                self.busy = Busy::Idle;
                self.status = e.user_message();
            }
            UiJobResult::Send(Ok(receipt)) => {
                self.busy = Busy::Idle;
                self.stage = Stage::Input;
                self.pending_tx = None;
                self.confirm_lines.clear();
                self.status = format!("LP tx ok ({})", receipt.hash);
            }
            UiJobResult::Send(Err(e)) => {
                self.busy = Busy::Idle;
                self.stage = Stage::Input;
                self.pending_tx = None;
                self.status = e.user_message();
            }
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let lines = match self.stage {
            Stage::Confirm => self.confirm_lines.clone(),
            Stage::Input => self.body_lines(),
        };
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(brand::body_color())),
            content,
        );
        let status = if self.busy != Busy::Idle {
            format!("{} {}", spinner_frame(self.tick), self.status)
        } else {
            self.status.clone()
        };
        frame.render_widget(status_paragraph(&status), status_area);
    }

    fn body_lines(&self) -> Vec<Line<'static>> {
        let mut out = vec![
            Line::from(vec![
                Span::styled("Tab ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!(
                    "{} · {} · {}   (←/→)",
                    Tab::List.label(),
                    Tab::Mint.label(),
                    Tab::Collect.label()
                )),
            ]),
            Line::from(format!(
                "Active: {} · chain {}",
                self.tab.label(),
                self.chain_id
            )),
        ];
        match self.tab {
            Tab::List => {
                if self.positions.is_empty() {
                    out.push(Line::from("(no positions — Mint tab or r reload)"));
                } else {
                    for (i, p) in self.positions.iter().enumerate() {
                        let mark = if i == self.sel { "▸" } else { " " };
                        out.push(Line::from(format!(
                            "{mark} #{} fee={} liq={} owed0={} owed1={}",
                            p.token_id,
                            p.fee,
                            p.liquidity,
                            p.tokens_owed0,
                            p.tokens_owed1
                        )));
                    }
                }
            }
            Tab::Mint => {
                out.push(Line::from("Mint new NPM position (full range ticks)"));
                out.push(Line::from(format!(
                    "token0: {}",
                    self.token0.value()
                )));
                out.push(Line::from(format!(
                    "token1: {}",
                    self.token1.value()
                )));
                out.push(Line::from(format!("fee: {}", self.fee.value())));
                out.push(Line::from(format!(
                    "amount0: {}",
                    self.amount0.value()
                )));
                out.push(Line::from(format!(
                    "amount1: {}",
                    self.amount1.value()
                )));
            }
            Tab::Collect => {
                if let Some(p) = self.positions.get(self.sel) {
                    out.push(Line::from(format!(
                        "Collect fees for NFT #{} (fee {})",
                        p.token_id, p.fee
                    )));
                } else {
                    out.push(Line::from("Select a position on List tab first"));
                }
            }
        }
        out
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &WalletState,
        handle: &Handle,
        events: &EventBus,
    ) -> KeyOutcome {
        if self.busy != Busy::Idle {
            return KeyOutcome::Consumed;
        }
        if self.stage == Stage::Confirm {
            return self.handle_confirm(key, wallet, handle, events);
        }
        match key.code {
            KeyCode::Left => {
                self.tab = self.tab.prev();
                KeyOutcome::Consumed
            }
            KeyCode::Right => {
                self.tab = self.tab.next();
                KeyOutcome::Consumed
            }
            KeyCode::Up if self.tab == Tab::List && !self.positions.is_empty() => {
                self.sel = self.sel.saturating_sub(1);
                KeyOutcome::Consumed
            }
            KeyCode::Down if self.tab == Tab::List && !self.positions.is_empty() => {
                if self.sel + 1 < self.positions.len() {
                    self.sel += 1;
                }
                KeyOutcome::Consumed
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if let Some(job) = self.list_job(wallet) {
                    self.busy = Busy::Loading;
                    self.status = "Loading positions…".into();
                    KeyOutcome::StartJob(job)
                } else {
                    self.status = "LP list requires chain 943".into();
                    KeyOutcome::Consumed
                }
            }
            KeyCode::Enter => self.submit(wallet),
            KeyCode::Esc => KeyOutcome::Back,
            _ => KeyOutcome::NotHandled,
        }
    }

    fn submit(&mut self, wallet: &WalletState) -> KeyOutcome {
        if self.chain_id != 943 {
            self.status = "Switch to PulseChain testnet (943)".into();
            return KeyOutcome::Consumed;
        }
        match self.tab {
            Tab::List => {
                self.tab = Tab::Collect;
                KeyOutcome::Consumed
            }
            Tab::Mint => match self.build_mint_tx(wallet) {
                Ok(tx) => {
                    self.pending_tx = Some(tx);
                    self.confirm_lines = vec![
                        Line::from("Confirm mint (Enter send · Esc cancel)"),
                        Line::from(format!("token0: {}", self.token0.value())),
                        Line::from(format!("token1: {}", self.token1.value())),
                        Line::from(format!("fee: {}", self.fee.value())),
                        Line::from(format!("amount0: {}", self.amount0.value())),
                        Line::from(format!("amount1: {}", self.amount1.value())),
                    ];
                    self.stage = Stage::Confirm;
                    KeyOutcome::Consumed
                }
                Err(e) => {
                    self.status = e;
                    KeyOutcome::Consumed
                }
            },
            Tab::Collect => match self.build_collect_tx(wallet) {
                Ok(tx) => {
                    self.pending_tx = Some(tx);
                    self.confirm_lines = vec![
                        Line::from("Confirm collect (Enter send · Esc cancel)"),
                        Line::from(format!(
                            "NFT #{}",
                            self.positions
                                .get(self.sel)
                                .map(|p| p.token_id.to_string())
                                .unwrap_or_default()
                        )),
                    ];
                    self.stage = Stage::Confirm;
                    KeyOutcome::Consumed
                }
                Err(e) => {
                    self.status = e;
                    KeyOutcome::Consumed
                }
            },
        }
    }

    fn build_mint_tx(&self, wallet: &WalletState) -> Result<vaughan_core::chains::EvmTransaction, String> {
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let rpc = wallet.active_rpc_url();
        let token0 = parse_token_address(self.token0.value(), "token0")?;
        let token1 = parse_token_address(self.token1.value(), "token1")?;
        let fee: u32 = self
            .fee
            .value()
            .trim()
            .parse()
            .map_err(|_| "Invalid fee tier".to_string())?;
        let amount0 = parse_swap_amount(self.amount0.value(), "amount0", 18)?;
        let amount1 = parse_swap_amount(self.amount1.value(), "amount1", 18)?;
        let (tick_lower, tick_upper) =
            default_full_range_ticks(fee).map_err(|e| e.user_message())?;
        let amount0_min = min_out_after_slippage(amount0, DEFAULT_DEX_SLIPPAGE_BPS);
        let amount1_min = min_out_after_slippage(amount1, DEFAULT_DEX_SLIPPAGE_BPS);
        build_v3_mint_evm(
            &from,
            self.chain_id,
            &rpc,
            token0,
            token1,
            fee,
            tick_lower,
            tick_upper,
            amount0,
            amount1,
            amount0_min,
            amount1_min,
            None,
        )
        .map_err(|e| e.user_message())
    }

    fn build_collect_tx(&self, wallet: &WalletState) -> Result<vaughan_core::chains::EvmTransaction, String> {
        let pos = self
            .positions
            .get(self.sel)
            .ok_or_else(|| "No position selected".to_string())?;
        let from = wallet
            .active_address()
            .map_err(|e| e.user_message())?
            .to_string();
        let rpc = wallet.active_rpc_url();
        build_v3_collect_evm(
            &from,
            self.chain_id,
            &rpc,
            pos.token_id,
            None,
            u128::MAX,
            u128::MAX,
        )
        .map_err(|e| e.user_message())
    }

    fn handle_confirm(
        &mut self,
        key: KeyEvent,
        _wallet: &WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                self.stage = Stage::Input;
                self.pending_tx = None;
                self.confirm_lines.clear();
                KeyOutcome::Consumed
            }
            KeyCode::Enter => {
                let Some(tx) = self.pending_tx.take() else {
                    self.stage = Stage::Input;
                    return KeyOutcome::Consumed;
                };
                self.busy = Busy::Sending;
                self.status = "Broadcasting…".into();
                KeyOutcome::StartJob(UiJob::SendEvm { tx })
            }
            _ => KeyOutcome::Consumed,
        }
    }
}
