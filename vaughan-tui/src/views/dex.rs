//! DEX swap view: any Uniswap V2– or V3–compatible router on PulseChain.
//!
//! When the DEX row is focused: **↑/↓** pick venue, **←/→** pick V2 or V3.
//! Calldata lives in [`dex_calldata`]. OTC / Balancer venues are listed but
//! not swap-wired yet (different ABIs).

use alloy::primitives::{Address, U256};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use std::str::FromStr;
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::dex_calldata::{
    build_approve_tx, build_swap_tx, encode_v3_path, hop_tokens, DexProtocol, DexSwapRequest,
};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

/// Common V3 fee tiers. Includes Pancake/wiz4rd `2500` and `20000` (2%).
const FEE_TIERS: &[u32] = &[100, 500, 2500, 3000, 10_000, 20_000];

/// PulseChain DEX venues (↑/↓). AMM Uni-forks get routers; OTC/Balancer are listed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DexVenue {
    /// Vaughan’s Pancake V3 fork on Pulse testnet (see `vaughan_core::core::wiz4rd`).
    Wiz4rd,
    PulseX,
    PulseXV1,
    NineMm,
    NineInch,
    SparkSwap,
    Dextop,
    UniHedron,
    PDex,
    Phux,
    Tide,
    FiDex,
    Bistro,
    AgoraX,
    Curv,
    Custom,
}

const VENUES: &[DexVenue] = &[
    DexVenue::Wiz4rd,
    DexVenue::PulseX,
    DexVenue::PulseXV1,
    DexVenue::NineMm,
    DexVenue::NineInch,
    DexVenue::SparkSwap,
    DexVenue::Dextop,
    DexVenue::UniHedron,
    DexVenue::PDex,
    DexVenue::Phux,
    DexVenue::Tide,
    DexVenue::FiDex,
    DexVenue::Bistro,
    DexVenue::AgoraX,
    DexVenue::Curv,
    DexVenue::Custom,
];

impl DexVenue {
    fn label(self) -> &'static str {
        match self {
            Self::Wiz4rd => "Wiz4rd",
            Self::PulseX => "PulseX",
            Self::PulseXV1 => "PulseX V1",
            Self::NineMm => "9mm",
            Self::NineInch => "9inch",
            Self::SparkSwap => "SparkSwap",
            Self::Dextop => "Dextop",
            Self::UniHedron => "Uniswap",
            Self::PDex => "pDex",
            Self::Phux => "PHUX",
            Self::Tide => "0xTide",
            Self::FiDex => "FiDex",
            Self::Bistro => "0xBistro",
            Self::AgoraX => "AgoraX",
            Self::Curv => "CURV",
            Self::Custom => "Custom",
        }
    }

    /// One-line blurb for status (what this venue is).
    fn blurb(self) -> &'static str {
        match self {
            Self::Wiz4rd => "Vaughan Pancake V3 fork · Pulse testnet 943",
            Self::PulseX => "largest PLS DEX · V2 AMM + V3 SwapRouter",
            Self::PulseXV1 => "legacy PulseX V1 AMM router",
            Self::NineMm => "Uni V3-fork concentrated liquidity",
            Self::NineInch => "V2 + V3 DEX (limit orders on site)",
            Self::SparkSwap => "dexSWAP / Spark Swap (V2-style)",
            Self::Dextop => "Uni V3-style · zkzx frontend",
            Self::UniHedron => "Uniswap V3 periphery on PulseChain",
            Self::PDex => "pDex V3 router",
            Self::Phux => "Balancer-style weighted pools — not wired",
            Self::Tide => "Balancer-fork dynamic fees — not wired",
            Self::FiDex => "Function Island — paste router (unknown)",
            Self::Bistro => "OTC — not AMM swap yet",
            Self::AgoraX => "OTC marketplace — not AMM swap yet",
            Self::Curv => "OTC + aggregator — not AMM swap yet",
            Self::Custom => "paste any Uni V2/V3-compatible router",
        }
    }

    /// Why Vaughan cannot build a swap tx yet (if any).
    fn unsupported(self) -> Option<&'static str> {
        match self {
            Self::Phux => Some("Balancer vault — need Balancer swap path"),
            Self::Tide => Some("Balancer-fork — need vault swap path"),
            Self::Bistro | Self::AgoraX | Self::Curv => Some("OTC desk — not an AMM router"),
            Self::FiDex => Some("no published router in catalog yet"),
            _ => None,
        }
    }

    fn next(self) -> Self {
        let i = VENUES.iter().position(|v| *v == self).unwrap_or(0);
        VENUES[(i + 1) % VENUES.len()]
    }

    fn prev(self) -> Self {
        let i = VENUES.iter().position(|v| *v == self).unwrap_or(0);
        VENUES[(i + VENUES.len() - 1) % VENUES.len()]
    }
}

impl DexProtocol {
    fn label(self) -> &'static str {
        match self {
            Self::V2 => "V2",
            Self::V3 => "V3",
        }
    }

    fn toggle(self) -> Self {
        match self {
            Self::V2 => Self::V3,
            Self::V3 => Self::V2,
        }
    }
}

/// WPLS / tWPLS for hop routing.
fn wpls_for_chain(chain_id: u64) -> &'static str {
    match chain_id {
        369 => "0xA1077a294dDE1B09bB078844df40758a5D0f9a27",
        943 => "0x70499adEBB11Efd915E3b69E700c331778628707",
        _ => "",
    }
}

fn chain_label(chain_id: u64) -> &'static str {
    match chain_id {
        369 => "PulseChain mainnet",
        943 => "PulseChain testnet",
        _ => "this network",
    }
}

/// Known Uni-compatible router for `(venue, protocol, chain)`.
///
/// Sources: PulseX docs, 9mm `deployments/pulsechain`, scan.pulsechain.com,
/// pulsechainramp `.env.example` (9inch V3, Dextop, pDex, Tide). Balancer /
/// OTC venues intentionally return `None`.
fn venue_router(venue: DexVenue, protocol: DexProtocol, chain_id: u64) -> Option<&'static str> {
    if venue.unsupported().is_some() {
        return None;
    }
    match (venue, protocol, chain_id) {
        // wiz4rd-swap — V3 only on Pulse testnet (docs/wiz4rd-addresses.md)
        (DexVenue::Wiz4rd, DexProtocol::V3, 943) => {
            Some(vaughan_core::core::wiz4rd::SWAP_ROUTER_943)
        }
        // PulseX
        (DexVenue::PulseX, DexProtocol::V2, 369) => {
            Some("0x165C3410fC91EF562C50559f7d2289fEbed552d9")
        }
        (DexVenue::PulseX, DexProtocol::V2, 943) => {
            Some("0xDaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7")
        }
        (DexVenue::PulseX, DexProtocol::V3, 369) => {
            Some("0xDA9aBA4eACF54E0273f56dfFee6B8F1e20B23Bba")
        }
        (DexVenue::PulseXV1, DexProtocol::V2, 369) => {
            Some("0x98bf93ebf5c380C0e6Ae8e192A7e2AE08edAcc02")
        }
        // 9mm — V2 factory router + V3 SwapRouter (not SmartRouter)
        (DexVenue::NineMm, DexProtocol::V2, 369) => {
            Some("0xcC73b59F8D7b7c532703bDfea2808a28a488cF47")
        }
        (DexVenue::NineMm, DexProtocol::V3, 369) => {
            Some("0x7bE8fbe502191bBBCb38b02f2d4fA0D628301bEA")
        }
        // 9inch
        (DexVenue::NineInch, DexProtocol::V2, 369) => {
            Some("0xeB45a3c4aedd0F47F345fB4c8A1802BB5740d725")
        }
        (DexVenue::NineInch, DexProtocol::V3, 369) => {
            Some("0x42556A17EF0Bd815bF21aD628DFd2e2f3b5F9ac7")
        }
        // SparkSwap / dexSWAP
        (DexVenue::SparkSwap, DexProtocol::V2, 369) => {
            Some("0x76C08825b4A675FD6a17A244660BabeB4ADA79d5")
        }
        // Dextop (+ zkzx UI) · pDex — V3-style
        (DexVenue::Dextop, DexProtocol::V3, 369) => {
            Some("0x1f849694Ef24a2245bCa415FE47500216B24d7FF")
        }
        (DexVenue::PDex, DexProtocol::V3, 369) => {
            Some("0x1eC2eaA62117486c9b2a05F098a7bF2568e19204")
        }
        // Bridged Uniswap V3 SwapRouter on PulseChain (Hedron frontend)
        (DexVenue::UniHedron, DexProtocol::V3, 369) => {
            Some("0xE592427A0AEce92De3Edee1F18E0157C05861564")
        }
        _ => None,
    }
}

fn missing_router_hint(venue: DexVenue, protocol: DexProtocol, chain_id: u64) -> String {
    if let Some(why) = venue.unsupported() {
        return format!("{} — {} · {}", venue.label(), venue.blurb(), why);
    }
    let other = protocol.toggle();
    let other_ok = venue_router(venue, other, chain_id).is_some();
    let mainnet_ok = chain_id != 369 && venue_router(venue, protocol, 369).is_some();
    let mut parts = vec![format!(
        "{} {} — no catalogued router on {}",
        venue.label(),
        protocol.label(),
        chain_label(chain_id)
    )];
    if other_ok {
        parts.push(format!("try ←/→ for {}", other.label()));
    }
    if mainnet_ok {
        parts.push("or Settings→Net → PulseChain mainnet".into());
    }
    parts.push("Custom = paste any Uni V2/V3 router".into());
    parts.join(" · ")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmStep {
    Approve,
    Swap,
}

enum Stage {
    Input,
    Confirm(ConfirmStep),
    Done,
}

#[derive(PartialEq, Eq)]
enum Focus {
    /// DEX venue + V2/V3 row (↑/↓ venue, ←/→ protocol).
    Dex,
    Router,
    TokenIn,
    TokenOut,
    Fee,
    Amount,
    MinOut,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Idle,
    Approving,
    Swapping,
}

pub struct DexView {
    stage: Stage,
    focus: Focus,
    venue: DexVenue,
    protocol: DexProtocol,
    fee: u32,
    router: Input,
    token_in: Input,
    token_out: Input,
    amount: Input,
    min_out: Input,
    native_in: bool,
    wpls: Option<Address>,
    chain_id: u64,
    busy: Busy,
    tick: u64,
    status: String,
    tx_hash: Option<String>,
    approve_hash: Option<String>,
    confirm_lines: Vec<String>,
}

impl Default for DexView {
    fn default() -> Self {
        Self {
            stage: Stage::Input,
            focus: Focus::Dex,
            venue: DexVenue::PulseX,
            protocol: DexProtocol::V2,
            fee: 3000,
            router: Input::new(false, "0x router…"),
            token_in: Input::new(false, "0x token in…"),
            token_out: Input::new(false, "0x token out…"),
            amount: Input::new(false, "amount in wei"),
            min_out: Input::new(false, "min out wei"),
            native_in: true,
            wpls: None,
            chain_id: 0,
            busy: Busy::Idle,
            tick: 0,
            status: String::new(),
            tx_hash: None,
            approve_hash: None,
            confirm_lines: Vec::new(),
        }
    }
}

impl DexView {
    /// Prefill from the active chain (Wiz4rd V3 on 943; PulseX V2 elsewhere).
    pub fn for_chain(chain_id: u64) -> Self {
        let mut v = Self {
            chain_id,
            ..Self::default()
        };
        if chain_id == 943 {
            v.venue = DexVenue::Wiz4rd;
            v.protocol = DexProtocol::V3;
            v.fee = 500;
        }
        v.apply_venue_defaults(true);
        v.status = match chain_id {
            943 => "Wiz4rd V3 (testnet) · ↑/↓ venue · ←/→ V2/V3 · f fee tier".into(),
            369 => "↑/↓ DEX · ←/→ V2/V3 · Space native · paste Custom router anytime".into(),
            _ => "↑/↓ DEX · ←/→ V2/V3 · paste a router address.".into(),
        };
        v
    }

    fn apply_venue_defaults(&mut self, overwrite_router: bool) {
        let wpls = wpls_for_chain(self.chain_id);
        if !wpls.is_empty() {
            if let Ok(addr) = Address::from_str(wpls) {
                self.wpls = Some(addr);
            }
            if self.native_in || self.token_in.value().trim().is_empty() {
                self.token_in.set_value(wpls);
            }
        }

        if self.venue == DexVenue::Custom && !overwrite_router {
            return;
        }

        // wiz4rd is V3-only — snap protocol when selecting the venue.
        if self.venue == DexVenue::Wiz4rd {
            self.protocol = DexProtocol::V3;
        }

        match venue_router(self.venue, self.protocol, self.chain_id) {
            Some(router) => {
                if overwrite_router || self.venue != DexVenue::Custom {
                    self.router.set_value(router);
                }
            }
            None if self.venue != DexVenue::Custom => {
                self.router.set_value("");
                self.status = missing_router_hint(self.venue, self.protocol, self.chain_id);
            }
            None => {}
        }
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::Send(Ok(hash)) => match self.busy {
                Busy::Approving => {
                    self.busy = Busy::Idle;
                    self.approve_hash = Some(hash.clone());
                    self.status = format!("Approve sent ({hash}). Confirm swap next.");
                    self.enter_confirm(ConfirmStep::Swap);
                }
                Busy::Swapping => {
                    self.busy = Busy::Idle;
                    self.tx_hash = Some(hash);
                    self.stage = Stage::Done;
                    self.status = "Swap broadcast.".into();
                }
                Busy::Idle => {}
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
            Busy::Approving => format!("{} approving router spend…", spinner_frame(self.tick)),
            Busy::Swapping => format!("{} broadcasting swap…", spinner_frame(self.tick)),
            Busy::Idle => self.status.clone(),
        };
        frame.render_widget(status_paragraph(&status_text), status);
    }

    fn render_input(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " DEX — PulseChain venues · any V2/V3 router ",
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD),
            ))),
            chunks[0],
        );

        let dex_style = if self.focus == Focus::Dex {
            Style::default()
                .fg(brand::accent_color())
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(brand::body_color())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("DEX {}  ·  {}  ", self.venue.label(), self.protocol.label()),
                    dex_style,
                ),
                Span::styled(
                    "↑/↓ venue  ←/→ V2/V3",
                    Style::default().fg(brand::body_color()),
                ),
            ])),
            chunks[1],
        );

        render_labeled_input(
            frame,
            chunks[2],
            "Router",
            &self.router,
            self.focus == Focus::Router,
        );
        render_labeled_input(
            frame,
            chunks[3],
            "Token in",
            &self.token_in,
            self.focus == Focus::TokenIn,
        );
        render_labeled_input(
            frame,
            chunks[4],
            "Token out",
            &self.token_out,
            self.focus == Focus::TokenOut,
        );

        let fee_style = if self.focus == Focus::Fee {
            Style::default()
                .fg(brand::accent_color())
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(brand::body_color())
        };
        let fee_line = if self.protocol == DexProtocol::V3 {
            format!("fee tier: {}  (Tab here · ←/→ cycle)", self.fee)
        } else {
            "fee tier: n/a (V2)".into()
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(fee_line, fee_style))),
            chunks[5],
        );

        render_labeled_input(
            frame,
            chunks[6],
            "Amount (wei)",
            &self.amount,
            self.focus == Focus::Amount,
        );
        render_labeled_input(
            frame,
            chunks[7],
            "Min out (wei)",
            &self.min_out,
            self.focus == Focus::MinOut,
        );

        let native = if self.native_in {
            "native IN (PLS→token)  [Space]"
        } else {
            "token↔token / meme↔meme  [Space]"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                native,
                Style::default().fg(brand::body_color()),
            ))),
            chunks[8],
        );
        frame.render_widget(
            Paragraph::new("Tab fields · Enter review · Esc home")
                .style(Style::default().fg(brand::body_color())),
            chunks[9],
        );
    }

    fn render_confirm(&self, frame: &mut Frame, area: Rect) {
        let title = match self.stage {
            Stage::Confirm(ConfirmStep::Approve) => " Confirm approve (1/2) ",
            Stage::Confirm(ConfirmStep::Swap) => {
                if self.native_in {
                    " Confirm swap "
                } else {
                    " Confirm swap (2/2) "
                }
            }
            _ => " Confirm ",
        };
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(title)));
        let mut lines: Vec<Line> = self
            .confirm_lines
            .iter()
            .map(|l| Line::from(l.clone()))
            .collect();
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Enter / y broadcast · Esc / n back",
            Style::default().fg(brand::accent_color()),
        )));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    }

    fn render_done(&self, frame: &mut Frame, area: Rect) {
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(" Swap sent ")));
        let mut lines = Vec::new();
        if let Some(a) = &self.approve_hash {
            lines.push(Line::from(format!("approve: {a}")));
        }
        let hash = self.tx_hash.as_deref().unwrap_or("(none)");
        lines.push(Line::from(format!("swap:    {hash}")));
        lines.push(Line::from(""));
        lines.push(Line::from("Enter new swap · Esc home"));
        frame.render_widget(Paragraph::new(lines), inner);
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
            Stage::Input => self.handle_input_key(key),
            Stage::Confirm(step) => self.handle_confirm_key(key, wallet, step),
            Stage::Done => match key.code {
                KeyCode::Enter => {
                    let chain_id = wallet.networks().active().chain_id;
                    *self = Self::for_chain(chain_id);
                    KeyOutcome::Consumed
                }
                KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
                _ => KeyOutcome::NotHandled,
            },
        }
    }

    fn cycle_fee(&mut self, forward: bool) {
        let Some(idx) = FEE_TIERS.iter().position(|f| *f == self.fee) else {
            self.fee = 3000;
            return;
        };
        let next = if forward {
            (idx + 1) % FEE_TIERS.len()
        } else {
            (idx + FEE_TIERS.len() - 1) % FEE_TIERS.len()
        };
        self.fee = FEE_TIERS[next];
    }

    fn handle_input_key(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
            KeyCode::Up | KeyCode::Down if self.focus == Focus::Dex => {
                self.venue = if matches!(key.code, KeyCode::Down) {
                    self.venue.next()
                } else {
                    self.venue.prev()
                };
                if venue_router(self.venue, self.protocol, self.chain_id).is_none()
                    && venue_router(self.venue, self.protocol.toggle(), self.chain_id).is_some()
                {
                    self.protocol = self.protocol.toggle();
                }
                self.apply_venue_defaults(true);
                if venue_router(self.venue, self.protocol, self.chain_id).is_some() {
                    self.status = format!(
                        "{} · {} — {}",
                        self.venue.label(),
                        self.protocol.label(),
                        self.venue.blurb()
                    );
                } else if self.venue == DexVenue::Custom {
                    self.status = format!(
                        "Custom · {} — paste any Uni V2/V3-compatible router",
                        self.protocol.label()
                    );
                }
                KeyOutcome::Consumed
            }
            KeyCode::Left | KeyCode::Right => {
                let forward = matches!(key.code, KeyCode::Right);
                match self.focus {
                    Focus::Dex => {
                        self.protocol = self.protocol.toggle();
                        let overwrite = self.venue != DexVenue::Custom;
                        self.apply_venue_defaults(overwrite);
                        if venue_router(self.venue, self.protocol, self.chain_id).is_some() {
                            self.status = format!(
                                "{} · {} — {}",
                                self.venue.label(),
                                self.protocol.label(),
                                self.venue.blurb()
                            );
                        } else if self.venue == DexVenue::Custom {
                            self.status = format!(
                                "Custom · {} — paste any Uni V2/V3-compatible router",
                                self.protocol.label()
                            );
                        }
                    }
                    Focus::Fee if self.protocol == DexProtocol::V3 => self.cycle_fee(forward),
                    _ => {}
                }
                KeyOutcome::Consumed
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Dex => Focus::Router,
                    Focus::Router => Focus::TokenIn,
                    Focus::TokenIn => Focus::TokenOut,
                    Focus::TokenOut => {
                        if self.protocol == DexProtocol::V3 {
                            Focus::Fee
                        } else {
                            Focus::Amount
                        }
                    }
                    Focus::Fee => Focus::Amount,
                    Focus::Amount => Focus::MinOut,
                    Focus::MinOut => Focus::Dex,
                };
                KeyOutcome::Consumed
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Dex => Focus::MinOut,
                    Focus::Router => Focus::Dex,
                    Focus::TokenIn => Focus::Router,
                    Focus::TokenOut => Focus::TokenIn,
                    Focus::Fee => Focus::TokenOut,
                    Focus::Amount => {
                        if self.protocol == DexProtocol::V3 {
                            Focus::Fee
                        } else {
                            Focus::TokenOut
                        }
                    }
                    Focus::MinOut => Focus::Amount,
                };
                KeyOutcome::Consumed
            }
            KeyCode::Char(' ') => {
                self.native_in = !self.native_in;
                KeyOutcome::Consumed
            }
            KeyCode::Enter => match self.validate_fields() {
                Ok(()) => {
                    let step = if self.native_in {
                        ConfirmStep::Swap
                    } else {
                        ConfirmStep::Approve
                    };
                    self.enter_confirm(step);
                    KeyOutcome::Consumed
                }
                Err(msg) => {
                    self.status = msg;
                    KeyOutcome::Consumed
                }
            },
            _ => {
                if matches!(self.focus, Focus::Dex | Focus::Fee) {
                    return KeyOutcome::Consumed;
                }
                let input = match self.focus {
                    Focus::Router => &mut self.router,
                    Focus::TokenIn => &mut self.token_in,
                    Focus::TokenOut => &mut self.token_out,
                    Focus::Amount => &mut self.amount,
                    Focus::MinOut => &mut self.min_out,
                    Focus::Dex | Focus::Fee => unreachable!(),
                };
                match input.handle_key(key) {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    InputAction::Consumed | InputAction::Submitted => KeyOutcome::Consumed,
                }
            }
        }
    }

    fn enter_confirm(&mut self, step: ConfirmStep) {
        let hops = self
            .parsed_hops()
            .map(|p| {
                p.iter()
                    .map(|a| format!("{a:#x}"))
                    .collect::<Vec<_>>()
                    .join(" → ")
            })
            .unwrap_or_else(|_| "(invalid path)".into());

        self.confirm_lines = match step {
            ConfirmStep::Approve => vec![
                format!(
                    "{} {} token→token: approve then swap",
                    self.venue.label(),
                    self.protocol.label()
                ),
                String::new(),
                format!("token:   {}", self.token_in.value().trim()),
                format!("spender: {}", self.router.value().trim()),
                format!("amount:  {} wei", self.amount.value().trim()),
                format!("path:    {hops}"),
            ],
            ConfirmStep::Swap => {
                let mut lines = vec![
                    format!(
                        "DEX:      {} ({})",
                        self.venue.label(),
                        self.protocol.label()
                    ),
                    format!("router:   {}", self.router.value().trim()),
                    format!("path:     {hops}"),
                ];
                if self.protocol == DexProtocol::V3 {
                    lines.push(format!("fee:      {}", self.fee));
                }
                lines.extend([
                    format!("amount:   {} wei", self.amount.value().trim()),
                    format!("min out:  {} wei", self.min_out.value().trim()),
                    format!(
                        "mode:     {}",
                        if self.native_in {
                            "native → token"
                        } else {
                            "token → token"
                        }
                    ),
                ]);
                if let Some(a) = &self.approve_hash {
                    lines.push(format!("approve:  {a}"));
                }
                lines
            }
        };
        self.stage = Stage::Confirm(step);
        if matches!(step, ConfirmStep::Approve) {
            self.status.clear();
        }
    }

    fn handle_confirm_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        step: ConfirmStep,
    ) -> KeyOutcome {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.stage = Stage::Input;
                KeyOutcome::Consumed
            }
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                let built = match step {
                    ConfirmStep::Approve => self.build_approve_job(wallet),
                    ConfirmStep::Swap => self.build_swap_job(wallet),
                };
                match built {
                    Ok(job) => {
                        self.busy = match step {
                            ConfirmStep::Approve => Busy::Approving,
                            ConfirmStep::Swap => Busy::Swapping,
                        };
                        KeyOutcome::StartJob(job)
                    }
                    Err(msg) => {
                        self.status = msg;
                        self.stage = Stage::Input;
                        KeyOutcome::Consumed
                    }
                }
            }
            _ => KeyOutcome::Consumed,
        }
    }

    fn parsed_hops(&self) -> Result<Vec<Address>, String> {
        let token_in = Address::from_str(self.token_in.value().trim())
            .map_err(|e| format!("bad token in: {e}"))?;
        let token_out = Address::from_str(self.token_out.value().trim())
            .map_err(|e| format!("bad token out: {e}"))?;
        hop_tokens(token_in, token_out, self.wpls, self.native_in)
    }

    fn validate_fields(&self) -> Result<(), String> {
        Address::from_str(self.router.value().trim()).map_err(|e| format!("bad router: {e}"))?;
        let hops = self.parsed_hops()?;
        if self.protocol == DexProtocol::V3 {
            let _ = encode_v3_path(&hops, self.fee)?;
        }
        let amount_in =
            U256::from_str(self.amount.value().trim()).map_err(|e| format!("bad amount: {e}"))?;
        let _ =
            U256::from_str(self.min_out.value().trim()).map_err(|e| format!("bad min out: {e}"))?;
        if amount_in.is_zero() {
            return Err("amount must be > 0".into());
        }
        Ok(())
    }

    fn build_approve_job(&self, wallet: &WalletState) -> Result<UiJob, String> {
        self.validate_fields()?;
        let router = Address::from_str(self.router.value().trim()).unwrap();
        let token_in = Address::from_str(self.token_in.value().trim()).unwrap();
        let amount_in = U256::from_str(self.amount.value().trim()).unwrap();
        let from = wallet.active_address().map_err(|e| e.user_message())?;
        let chain_id = wallet.networks().active().chain_id;
        Ok(UiJob::SendEvm {
            tx: build_approve_tx(token_in, router, amount_in, from, chain_id),
        })
    }

    fn build_swap_job(&self, wallet: &WalletState) -> Result<UiJob, String> {
        self.validate_fields()?;
        let router = Address::from_str(self.router.value().trim()).unwrap();
        let token_in = Address::from_str(self.token_in.value().trim()).unwrap();
        let token_out = Address::from_str(self.token_out.value().trim()).unwrap();
        let amount_in = U256::from_str(self.amount.value().trim()).unwrap();
        let min_out = U256::from_str(self.min_out.value().trim()).unwrap();
        let to_addr = wallet.active_address().map_err(|e| e.user_message())?;
        let recipient = Address::from_str(to_addr).map_err(|e| format!("bad account: {e}"))?;
        let chain_id = wallet.networks().active().chain_id;

        let tx = build_swap_tx(&DexSwapRequest {
            protocol: self.protocol,
            router,
            token_in,
            token_out,
            wpls: self.wpls,
            native_in: self.native_in,
            amount_in,
            min_out,
            fee: self.fee,
            recipient,
            from: to_addr.to_string(),
            chain_id,
        })?;
        Ok(UiJob::SendEvm { tx })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn venue_cycle_starts_pulsex_ends_custom() {
        assert_eq!(DexVenue::Wiz4rd.next(), DexVenue::PulseX);
        assert_eq!(DexVenue::PulseX.next(), DexVenue::PulseXV1);
        assert_eq!(DexVenue::NineMm.next(), DexVenue::NineInch);
        assert_eq!(DexVenue::Curv.next(), DexVenue::Custom);
        assert_eq!(DexVenue::Custom.next(), DexVenue::Wiz4rd);
        assert_eq!(VENUES.len(), 16);
    }

    #[test]
    fn pulsex_has_v2_and_v3_mainnet() {
        assert!(venue_router(DexVenue::PulseX, DexProtocol::V2, 369).is_some());
        assert_eq!(
            venue_router(DexVenue::PulseX, DexProtocol::V3, 369),
            Some("0xDA9aBA4eACF54E0273f56dfFee6B8F1e20B23Bba")
        );
    }

    #[test]
    fn nine_mm_v3_mainnet_only() {
        assert!(venue_router(DexVenue::NineMm, DexProtocol::V3, 369).is_some());
        assert!(venue_router(DexVenue::NineMm, DexProtocol::V3, 943).is_none());
        let hint = missing_router_hint(DexVenue::NineMm, DexProtocol::V3, 943);
        assert!(hint.contains("testnet"));
        assert!(hint.contains("mainnet"));
    }

    #[test]
    fn nine_mm_has_v2_and_v3_mainnet() {
        assert!(venue_router(DexVenue::NineMm, DexProtocol::V2, 369).is_some());
        assert_eq!(
            venue_router(DexVenue::NineMm, DexProtocol::V3, 369),
            Some("0x7bE8fbe502191bBBCb38b02f2d4fA0D628301bEA")
        );
    }

    #[test]
    fn nine_inch_v2_and_v3_routers() {
        assert_eq!(
            venue_router(DexVenue::NineInch, DexProtocol::V2, 369),
            Some("0xeB45a3c4aedd0F47F345fB4c8A1802BB5740d725")
        );
        assert_eq!(
            venue_router(DexVenue::NineInch, DexProtocol::V3, 369),
            Some("0x42556A17EF0Bd815bF21aD628DFd2e2f3b5F9ac7")
        );
    }

    #[test]
    fn spark_dextop_uni_catalogued() {
        assert!(venue_router(DexVenue::SparkSwap, DexProtocol::V2, 369).is_some());
        assert!(venue_router(DexVenue::Dextop, DexProtocol::V3, 369).is_some());
        assert!(venue_router(DexVenue::UniHedron, DexProtocol::V3, 369).is_some());
    }

    #[test]
    fn balancer_and_otc_are_listed_but_unsupported() {
        assert!(DexVenue::Phux.unsupported().is_some());
        assert!(DexVenue::Tide.unsupported().is_some());
        assert!(DexVenue::Bistro.unsupported().is_some());
        assert!(venue_router(DexVenue::Phux, DexProtocol::V2, 369).is_none());
        let hint = missing_router_hint(DexVenue::Phux, DexProtocol::V2, 369);
        assert!(hint.contains("Balancer"));
    }
}
