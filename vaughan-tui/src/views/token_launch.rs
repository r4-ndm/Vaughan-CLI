//! Launch a fixed-supply testnet meme coin (name + ticker + supply).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Gauge, Paragraph},
    Frame,
};
use vaughan_core::core::{
    token_launch_allowed, validate_token_name, validate_token_symbol, TokenLaunchOutcome,
    WalletState, TOKEN_LAUNCH_DECIMALS,
};
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Input,
    Confirm,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Name,
    Symbol,
    Supply,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Idle,
    Deploying,
}

pub struct TokenLaunchView {
    name: Input,
    symbol: Input,
    supply: Input,
    focus: Focus,
    stage: Stage,
    busy: Busy,
    tick: u64,
    status: String,
    confirm_lines: Vec<String>,
    outcome: Option<TokenLaunchOutcome>,
    chain_id: u64,
    allowed: bool,
}

impl TokenLaunchView {
    pub fn for_chain(chain_id: u64) -> Self {
        let allowed = token_launch_allowed(chain_id);
        Self {
            name: Input::new(false, "My Coin"),
            symbol: Input::new(false, "MEME"),
            supply: Input::new(false, "1000000000"),
            focus: Focus::Name,
            stage: Stage::Input,
            busy: Busy::Idle,
            tick: 0,
            status: if allowed {
                "Tab field · Enter confirm · Esc back".into()
            } else {
                "Token launch is testnet-only (switch to Pulse testnet v4)".into()
            },
            confirm_lines: Vec::new(),
            outcome: None,
            chain_id,
            allowed,
        }
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    /// Enter deploying state (TUI confirm or MCP approval → background job).
    pub fn begin_deploying(&mut self, name: String, symbol: String, supply: String) {
        self.name.set_value(name);
        self.symbol.set_value(symbol);
        self.supply.set_value(supply.clone());
        let supply_line = format!("Supply: {supply} (decimals {TOKEN_LAUNCH_DECIMALS})");
        self.confirm_lines = vec![
            format!("Name: {}", self.name.value().trim()),
            format!("Ticker: {}", self.symbol.value().trim()),
            supply_line,
            String::new(),
        ];
        self.stage = Stage::Confirm;
        self.busy = Busy::Deploying;
        self.status = format!(
            "Deploying {} ({})…",
            self.symbol.value().trim(),
            self.name.value().trim()
        );
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::DeployToken(Ok(outcome)) => {
                self.busy = Busy::Idle;
                self.stage = Stage::Done;
                self.outcome = Some(outcome.clone());
                self.status = format!(
                    "Launched {} ({}) at {}",
                    outcome.token.symbol, outcome.token.name, outcome.token.address
                );
            }
            UiJobResult::DeployToken(Err(e)) => {
                self.busy = Busy::Idle;
                self.stage = Stage::Input;
                self.status = e.user_message();
            }
            _ => {}
        }
    }

    pub fn launched_address(&self) -> Option<String> {
        self.outcome.as_ref().map(|o| o.token.address.clone())
    }

    /// Indeterminate deploy progress (ping-pong 0..1), same family as unlock KDF bar.
    fn deploy_progress(tick: u64) -> f64 {
        const CYCLE: u64 = 80;
        let phase = (tick % CYCLE) as f64 / CYCLE as f64;
        1.0 - (phase * 2.0 - 1.0).abs()
    }

    fn render_deploying(&self, frame: &mut Frame, content: Rect) {
        let [summary, bar_area, hint] = Layout::vertical([
            Constraint::Min(6),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .areas(content);
        let inner =
            brand::render_faded_box(frame, summary, Some(brand::fade_line(" Deploying token ")));
        let mut lines: Vec<Line> = self
            .confirm_lines
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| Line::from(s.as_str()))
            .collect();
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Signing, broadcasting, and waiting for confirmation…",
            Style::default().add_modifier(Modifier::ITALIC),
        )));
        frame.render_widget(Paragraph::new(lines), inner);
        let bar_inner = brand::render_faded_box(frame, bar_area, None);
        let gauge = Gauge::default()
            .ratio(Self::deploy_progress(self.tick))
            .gauge_style(
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )
            .label("");
        frame.render_widget(gauge, bar_inner);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Please wait — deploy in progress",
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Center),
            hint,
        );
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        if matches!(self.busy, Busy::Deploying) {
            self.render_deploying(frame, content);
            let status = format!(
                "{} deploying {}…",
                spinner_frame(self.tick),
                self.symbol.value().trim()
            );
            frame.render_widget(status_paragraph(&status), status_area);
            return;
        }
        match self.stage {
            Stage::Done => {
                let inner = brand::render_faded_box(
                    frame,
                    content,
                    Some(brand::fade_line(" Token launched ")),
                );
                let lines: Vec<Line> = if let Some(o) = &self.outcome {
                    vec![
                        Line::from(format!("{} ({})", o.token.symbol, o.token.name)),
                        Line::from(format!("Address: {}", o.token.address)),
                        Line::from(format!("Supply: {} (18 decimals)", self.supply.value())),
                        Line::from(format!("Tx: {}", o.tx_hash)),
                        Line::from(""),
                        Line::from("y — copy token address · Esc back"),
                    ]
                } else {
                    vec![Line::from("Done")]
                };
                frame.render_widget(Paragraph::new(lines), inner);
            }
            Stage::Confirm => {
                let inner = brand::render_faded_box(
                    frame,
                    content,
                    Some(brand::fade_line(" Confirm launch ")),
                );
                let lines: Vec<Line> = self
                    .confirm_lines
                    .iter()
                    .map(|s| Line::from(s.as_str()))
                    .collect();
                frame.render_widget(Paragraph::new(lines), inner);
            }
            Stage::Input => {
                let [head, fields] =
                    Layout::vertical([Constraint::Min(5), Constraint::Min(12)]).areas(content);
                let head_inner =
                    brand::render_faded_box(frame, head, Some(brand::fade_line(" Launch token ")));
                let net = wallet.networks().active();
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from("Fixed-supply ERC-20 · 18 decimals · mint to your wallet"),
                        Line::from(format!("Network: {} (chain {})", net.name, net.chain_id)),
                        Line::from("No verification step — testnet play only."),
                        Line::from(Span::styled(
                            "Anyone can deploy a token; liquidity is not guaranteed.",
                            Style::default().add_modifier(Modifier::ITALIC),
                        )),
                    ]),
                    head_inner,
                );
                let [n, s, q] = Layout::vertical([
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(3),
                ])
                .spacing(0)
                .areas(fields);
                render_labeled_input(frame, n, "Name", &self.name, self.focus == Focus::Name);
                render_labeled_input(
                    frame,
                    s,
                    "Ticker",
                    &self.symbol,
                    self.focus == Focus::Symbol,
                );
                render_labeled_input(
                    frame,
                    q,
                    "Supply",
                    &self.supply,
                    self.focus == Focus::Supply,
                );
            }
        }
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        matches!(self.stage, Stage::Done)
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &WalletState,
        _handle: &tokio::runtime::Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        if matches!(self.busy, Busy::Deploying) {
            return KeyOutcome::Consumed;
        }
        if !self.allowed {
            return match key.code {
                KeyCode::Esc => KeyOutcome::Back,
                _ => KeyOutcome::Consumed,
            };
        }
        match self.stage {
            Stage::Done => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(addr) = self.launched_address() {
                        match crate::clipboard::copy_text(&addr) {
                            Ok(()) => KeyOutcome::Flash("Token address copied".into()),
                            Err(e) => KeyOutcome::Flash(e),
                        }
                    } else {
                        KeyOutcome::Consumed
                    }
                }
                KeyCode::Esc => KeyOutcome::Back,
                _ => KeyOutcome::Consumed,
            },
            Stage::Confirm => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::Input;
                    KeyOutcome::Consumed
                }
                KeyCode::Enter | KeyCode::Char('y') => self.confirm_deploy(),
                _ => KeyOutcome::Consumed,
            },
            Stage::Input => {
                if key.code == KeyCode::Esc {
                    return KeyOutcome::Back;
                }
                if key.code == KeyCode::Tab {
                    self.focus = match self.focus {
                        Focus::Name => Focus::Symbol,
                        Focus::Symbol => Focus::Supply,
                        Focus::Supply => Focus::Name,
                    };
                    return KeyOutcome::Consumed;
                }
                let input = match self.focus {
                    Focus::Name => &mut self.name,
                    Focus::Symbol => &mut self.symbol,
                    Focus::Supply => &mut self.supply,
                };
                match input.handle_key(key) {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    InputAction::Consumed => KeyOutcome::Consumed,
                    InputAction::Submitted => self.begin_confirm(wallet),
                }
            }
        }
    }

    fn begin_confirm(&mut self, wallet: &WalletState) -> KeyOutcome {
        let name = match validate_token_name(self.name.value()) {
            Ok(s) => s,
            Err(e) => {
                self.status = e.user_message();
                return KeyOutcome::Consumed;
            }
        };
        let symbol = match validate_token_symbol(self.symbol.value()) {
            Ok(s) => s,
            Err(e) => {
                self.status = e.user_message();
                return KeyOutcome::Consumed;
            }
        };
        let supply = self.supply.value().trim().to_string();
        if supply.is_empty() {
            self.status = "supply required".into();
            return KeyOutcome::Consumed;
        }
        let from = match wallet.active_address() {
            Ok(a) => a.to_string(),
            Err(e) => {
                self.status = e.user_message();
                return KeyOutcome::Consumed;
            }
        };
        self.confirm_lines = vec![
            format!("Name: {name}"),
            format!("Ticker: {symbol}"),
            format!("Supply: {supply} (decimals {TOKEN_LAUNCH_DECIMALS})"),
            format!("Mint to: {from}"),
            format!(
                "Chain: {} ({})",
                wallet.networks().active().name,
                self.chain_id
            ),
            String::new(),
            "Enter / y deploy · Esc cancel".into(),
        ];
        self.stage = Stage::Confirm;
        KeyOutcome::Consumed
    }

    fn confirm_deploy(&mut self) -> KeyOutcome {
        self.busy = Busy::Deploying;
        self.status = format!(
            "Deploying {} ({})…",
            self.symbol.value().trim(),
            self.name.value().trim()
        );
        KeyOutcome::StartJob(UiJob::DeployToken {
            name: self.name.value().trim().to_string(),
            symbol: self.symbol.value().trim().to_string(),
            supply: self.supply.value().trim().to_string(),
        })
    }
}
