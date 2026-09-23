//! DCA view — list / create / pause recurring native→token buys.
//!
//! Softkey `o`. Thin shell over `vaughan_agent::dca` (no quote/swap math here).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph},
    Frame,
};
use vaughan_agent::{
    add_plan, agg_picker_labels, build_plan, cancel_plan, dex_picker_slugs, load_plans, now_unix,
    set_paused, DcaPlan, DcaStatus, DcaVenue, MIN_INTERVAL_SECS,
};
use vaughan_core::chains::Balance;
use vaughan_core::core::{parse_native_amount, DexProtocol, WalletState};
use vaughan_core::error::WalletError;
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::{
    body_areas, cycle_token_picker, manual_edit_resets_token_pick, parse_token_address,
    status_paragraph, TOKEN_PICK_UNINIT,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    List,
    Create,
    ConfirmCreate,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CreateFocus {
    Token,
    Amount,
    Interval,
    Route,
    Venue,
    MaxSlices,
    DryRun,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RouteKind {
    Agg,
    Dex,
}

const INTERVAL_PRESETS: &[(u64, &str)] = &[
    (MIN_INTERVAL_SECS, "15m"),
    (3600, "1h"),
    (4 * 3600, "4h"),
    (12 * 3600, "12h"),
    (24 * 3600, "24h"),
];

pub struct DcaView {
    plans: Vec<DcaPlan>,
    selected: usize,
    stage: Stage,
    tick: u64,
    status: String,
    busy: bool,
    profile_dir: std::path::PathBuf,
    /// Sentient auto-fire enabled for this session.
    auto_fire: bool,
    create_focus: CreateFocus,
    token_out: Input,
    amount: Input,
    interval_idx: usize,
    route_kind: RouteKind,
    agg_idx: usize,
    dex_idx: usize,
    max_slices: Input,
    dry_run: bool,
    token_pick: usize,
    pending_plan: Option<DcaPlan>,
    /// In-flight slice (avoid stacking jobs).
    fire_inflight: bool,
    /// Active chain for DEX venue picker.
    chain_id: u64,
}

impl DcaView {
    pub fn new(profile_dir: std::path::PathBuf, auto_fire: bool) -> Self {
        let mut v = Self {
            plans: Vec::new(),
            selected: 0,
            stage: Stage::List,
            tick: 0,
            status: String::new(),
            busy: false,
            profile_dir,
            auto_fire,
            create_focus: CreateFocus::Token,
            token_out: Input::new(false, "↑↓ pick token or paste 0x…"),
            amount: Input::new(false, "e.g. 100"),
            interval_idx: 2, // 4h
            route_kind: RouteKind::Agg,
            agg_idx: 0,
            dex_idx: 0,
            max_slices: {
                let mut i = Input::new(false, "e.g. 10");
                i.set_value("10");
                i
            },
            dry_run: true,
            token_pick: TOKEN_PICK_UNINIT,
            pending_plan: None,
            fire_inflight: false,
            chain_id: 369,
        };
        v.reload();
        v
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    /// Keep DEX picker in sync with the wallet's active network.
    pub fn set_chain_id(&mut self, chain_id: u64) {
        if self.chain_id != chain_id {
            self.chain_id = chain_id;
            self.dex_idx = 0;
        }
    }

    pub fn reload(&mut self) {
        match load_plans(&self.profile_dir) {
            Ok(file) => {
                self.plans = file.plans;
                if self.selected >= self.plans.len() {
                    self.selected = self.plans.len().saturating_sub(1);
                }
                self.status = if self.plans.is_empty() {
                    format!(
                        "No DCA plans · n new · Esc back{}",
                        if self.auto_fire {
                            " · Sentient auto-fire on"
                        } else {
                            " · open Sentient profile to auto-fire"
                        }
                    )
                } else {
                    format!(
                        "{} plan(s) · ↑↓ · p pause · c cancel · n new · Esc back",
                        self.plans.len()
                    )
                };
            }
            Err(e) => {
                self.status = e.to_string();
            }
        }
    }

    /// Background job to fire one due slice (caller checks Sentient).
    /// Prefer App::poll_dca_due — kept for tests / manual trigger.
    pub fn poll_fire_job(&mut self, breaker: vaughan_agent::CircuitBreaker) -> Option<UiJob> {
        if !self.auto_fire || self.fire_inflight {
            return None;
        }
        let now = now_unix();
        let due = vaughan_agent::poll_due_id(&self.profile_dir, now)
            .ok()
            .flatten()?;
        self.fire_inflight = true;
        self.status = format!("firing DCA `{due}`…");
        Some(UiJob::DcaSlice {
            plan_id: due,
            profile_dir: self.profile_dir.clone(),
            breaker,
        })
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        if let UiJobResult::DcaSlice(res) = result {
            self.fire_inflight = false;
            self.busy = false;
            match res {
                Ok(r) => {
                    self.status = if r.ok {
                        if r.dry_run {
                            format!("DCA {} dry-run ok", r.plan_id)
                        } else {
                            format!("DCA {} · {}", r.plan_id, r.tx_hash)
                        }
                    } else {
                        format!("DCA {} failed: {}", r.plan_id, r.error)
                    };
                    self.reload();
                }
                Err(e) => {
                    self.status = e.user_message();
                    self.reload();
                }
            }
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState, now: u64) {
        let _ = wallet;
        let [content, status_area] = body_areas(area);
        match self.stage {
            Stage::List => self.render_list(frame, content, now),
            Stage::Create => self.render_create(frame, content),
            Stage::ConfirmCreate => self.render_confirm(frame, content),
        }
        let status = if self.busy || self.fire_inflight {
            format!("{} {}", spinner_frame(self.tick), self.status)
        } else {
            self.status.clone()
        };
        frame.render_widget(status_paragraph(&status), status_area);
    }

    fn render_list(&self, frame: &mut Frame, area: Rect, now: u64) {
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(" DCA plans ")));
        let mut items = Vec::new();
        if self.plans.is_empty() {
            items.push(ListItem::new(Line::from("— no plans — press n to create")));
        } else {
            for (i, p) in self.plans.iter().enumerate() {
                let mark = if i == self.selected { "›" } else { " " };
                let due = if p.status == DcaStatus::Active {
                    if now >= p.next_due_at {
                        "due now".to_string()
                    } else {
                        let left = p.next_due_at.saturating_sub(now);
                        format!("in {}", format_duration(left))
                    }
                } else {
                    p.status.as_str().to_string()
                };
                let interval = p
                    .trigger
                    .interval_secs()
                    .map(format_duration)
                    .unwrap_or_else(|| "?".into());
                let dry = if p.dry_run { " · dry" } else { "" };
                let tok = short_addr(&p.token_out);
                let venue = p.venue.display_label();
                items.push(ListItem::new(Line::from(format!(
                    "{mark} {} · {tok} · {venue} · every {interval} · {}/{} · {due}{dry}",
                    p.id, p.slices_done, p.max_slices
                ))));
                if i == self.selected {
                    if let Some(last) = p.slice_log.last() {
                        let detail = if last.ok {
                            if last.dry_run {
                                "last: dry-run ok".to_string()
                            } else {
                                format!("last: {}", short_hash(&last.tx_hash))
                            }
                        } else {
                            format!("last err: {}", truncate(&last.error, 40))
                        };
                        items.push(ListItem::new(Line::from(Span::styled(
                            format!("    {detail}"),
                            Style::default().fg(ratatui::style::Color::DarkGray),
                        ))));
                    }
                }
            }
        }
        frame.render_widget(List::new(items), inner);
    }

    fn render_create(&self, frame: &mut Frame, area: Rect) {
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(" New DCA ")));
        let [tok, amt, iv, route, venue, mx, dry] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .areas(inner);
        crate::views::render_labeled_input(
            frame,
            tok,
            "Token out",
            &self.token_out,
            self.create_focus == CreateFocus::Token,
        );
        crate::views::render_labeled_input(
            frame,
            amt,
            "PLS per slice",
            &self.amount,
            self.create_focus == CreateFocus::Amount,
        );
        let iv_label = INTERVAL_PRESETS
            .get(self.interval_idx)
            .map(|(_, l)| *l)
            .unwrap_or("?");
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" Interval: {iv_label}  (←/→)"),
                focus_style(self.create_focus == CreateFocus::Interval),
            ))),
            iv,
        );
        let route_label = match self.route_kind {
            RouteKind::Agg => "Aggregator",
            RouteKind::Dex => "DEX (direct)",
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" Route: {route_label}  (←/→)  · use DEX if token missing on aggs"),
                focus_style(self.create_focus == CreateFocus::Route),
            ))),
            route,
        );
        let venue_label = self.current_venue_label();
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" Venue: {venue_label}  (←/→)"),
                focus_style(self.create_focus == CreateFocus::Venue),
            ))),
            venue,
        );
        crate::views::render_labeled_input(
            frame,
            mx,
            "Max slices",
            &self.max_slices,
            self.create_focus == CreateFocus::MaxSlices,
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(
                    " Dry-run: {}  (space toggle)",
                    if self.dry_run { "yes" } else { "NO — live" }
                ),
                focus_style(self.create_focus == CreateFocus::DryRun),
            ))),
            dry,
        );
    }

    fn current_venue_label(&self) -> String {
        match self.route_kind {
            RouteKind::Agg => agg_picker_labels()
                .get(self.agg_idx)
                .copied()
                .unwrap_or("auto")
                .to_string(),
            RouteKind::Dex => {
                let slugs = dex_picker_slugs(self.chain_id, DexProtocol::V2);
                slugs.get(self.dex_idx).copied().unwrap_or("—").to_string()
            }
        }
    }

    fn render_confirm(&self, frame: &mut Frame, area: Rect) {
        let inner =
            brand::render_faded_box(frame, area, Some(brand::fade_line(" Confirm DCA plan ")));
        let Some(p) = &self.pending_plan else {
            return;
        };
        let interval = p
            .trigger
            .interval_secs()
            .map(format_duration)
            .unwrap_or_else(|| "?".into());
        let lines = vec![
            Line::from(format!(
                "Buy {} every {interval} · chain {}",
                short_addr(&p.token_out),
                p.chain_id
            )),
            Line::from(format!("Wallet: {}", short_addr(&p.account))),
            Line::from(format!(
                "Slice: {} · max {} · slip {} bps",
                format_slice_human(&p.slice_amount_wei),
                p.max_slices,
                p.max_slippage_bps
            )),
            Line::from(format!(
                "Budget ≤ {} · Route: {} · dry_run: {}",
                format_budget_human(p),
                p.venue.display_label(),
                p.dry_run
            )),
            Line::from(""),
            Line::from("Enter — save plan · Esc — back"),
        ];
        frame.render_widget(Paragraph::new(lines), inner);
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        matches!(self.stage, Stage::List)
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        _handle: &tokio::runtime::Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        self.set_chain_id(wallet.networks().active().chain_id);
        let decimals = wallet.networks().active().decimals;
        let empty: &[Balance] = &[];
        let account = wallet
            .active_address()
            .ok()
            .and_then(|s| s.parse::<alloy::primitives::Address>().ok());
        match self.stage {
            Stage::List => self.handle_list(key),
            Stage::Create => self.handle_create(key, empty, decimals, account),
            Stage::ConfirmCreate => self.handle_confirm(key),
        }
    }

    pub fn handle_key_with_assets(
        &mut self,
        key: KeyEvent,
        assets: &[Balance],
        native_decimals: u8,
        account: Option<alloy::primitives::Address>,
    ) -> KeyOutcome {
        match self.stage {
            Stage::List => self.handle_list(key),
            Stage::Create => self.handle_create(key, assets, native_decimals, account),
            Stage::ConfirmCreate => self.handle_confirm(key),
        }
    }

    fn handle_list(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
            KeyCode::Up | KeyCode::Char('k') => {
                if !self.plans.is_empty() {
                    self.selected = self.selected.saturating_sub(1);
                }
                KeyOutcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.plans.is_empty() {
                    self.selected = (self.selected + 1).min(self.plans.len().saturating_sub(1));
                }
                KeyOutcome::Consumed
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.stage = Stage::Create;
                self.create_focus = CreateFocus::Token;
                self.status =
                    "Tab fields · ←/→ route & venue · Enter confirm draft · Esc cancel".into();
                KeyOutcome::Consumed
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                if let Some(p) = self.plans.get(self.selected) {
                    let pause = p.status == DcaStatus::Active;
                    match set_paused(&self.profile_dir, &p.id, pause) {
                        Ok(_) => self.reload(),
                        Err(e) => self.status = e.to_string(),
                    }
                }
                KeyOutcome::Consumed
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                if let Some(p) = self.plans.get(self.selected) {
                    match cancel_plan(&self.profile_dir, &p.id) {
                        Ok(_) => self.reload(),
                        Err(e) => self.status = e.to_string(),
                    }
                }
                KeyOutcome::Consumed
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.reload();
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::NotHandled,
        }
    }

    fn handle_create(
        &mut self,
        key: KeyEvent,
        assets: &[Balance],
        native_decimals: u8,
        account: Option<alloy::primitives::Address>,
    ) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                self.stage = Stage::List;
                self.reload();
                KeyOutcome::Consumed
            }
            KeyCode::Tab => {
                self.create_focus = match self.create_focus {
                    CreateFocus::Token => CreateFocus::Amount,
                    CreateFocus::Amount => CreateFocus::Interval,
                    CreateFocus::Interval => CreateFocus::Route,
                    CreateFocus::Route => CreateFocus::Venue,
                    CreateFocus::Venue => CreateFocus::MaxSlices,
                    CreateFocus::MaxSlices => CreateFocus::DryRun,
                    CreateFocus::DryRun => CreateFocus::Token,
                };
                KeyOutcome::Consumed
            }
            KeyCode::Left | KeyCode::Right
                if matches!(
                    self.create_focus,
                    CreateFocus::Interval | CreateFocus::Route | CreateFocus::Venue
                ) =>
            {
                let forward = matches!(key.code, KeyCode::Right);
                match self.create_focus {
                    CreateFocus::Interval => {
                        if forward {
                            if self.interval_idx + 1 < INTERVAL_PRESETS.len() {
                                self.interval_idx += 1;
                            }
                        } else if self.interval_idx > 0 {
                            self.interval_idx -= 1;
                        }
                    }
                    CreateFocus::Route => {
                        self.route_kind = match self.route_kind {
                            RouteKind::Agg => RouteKind::Dex,
                            RouteKind::Dex => RouteKind::Agg,
                        };
                    }
                    CreateFocus::Venue => self.cycle_venue(forward),
                    _ => {}
                }
                KeyOutcome::Consumed
            }
            KeyCode::Char(' ') if self.create_focus == CreateFocus::DryRun => {
                self.dry_run = !self.dry_run;
                KeyOutcome::Consumed
            }
            KeyCode::Up | KeyCode::Down if self.create_focus == CreateFocus::Token => {
                let mut native = false;
                cycle_token_picker(
                    assets,
                    true,
                    &mut self.token_pick,
                    matches!(key.code, KeyCode::Down),
                    &mut native,
                    &mut self.token_out,
                    &mut self.status,
                );
                KeyOutcome::Consumed
            }
            KeyCode::Enter => match self.draft_plan(native_decimals, account) {
                Ok(plan) => {
                    self.pending_plan = Some(plan);
                    self.stage = Stage::ConfirmCreate;
                    self.status = "Enter save · Esc edit".into();
                    KeyOutcome::Consumed
                }
                Err(e) => {
                    self.status = e;
                    KeyOutcome::Consumed
                }
            },
            code => {
                let input = match self.create_focus {
                    CreateFocus::Token => &mut self.token_out,
                    CreateFocus::Amount => &mut self.amount,
                    CreateFocus::MaxSlices => &mut self.max_slices,
                    CreateFocus::Interval
                    | CreateFocus::Route
                    | CreateFocus::Venue
                    | CreateFocus::DryRun => {
                        return KeyOutcome::NotHandled;
                    }
                };
                if manual_edit_resets_token_pick(code) && self.create_focus == CreateFocus::Token {
                    self.token_pick = TOKEN_PICK_UNINIT;
                }
                match input.handle_key(key) {
                    InputAction::Consumed => KeyOutcome::Consumed,
                    InputAction::Submitted => KeyOutcome::Consumed,
                    InputAction::Ignored => KeyOutcome::NotHandled,
                }
            }
        }
    }

    fn cycle_venue(&mut self, forward: bool) {
        match self.route_kind {
            RouteKind::Agg => {
                let n = agg_picker_labels().len().max(1);
                if forward {
                    self.agg_idx = (self.agg_idx + 1) % n;
                } else {
                    self.agg_idx = (self.agg_idx + n - 1) % n;
                }
            }
            RouteKind::Dex => {
                let n = dex_picker_slugs(self.chain_id, DexProtocol::V2)
                    .len()
                    .max(1);
                if forward {
                    self.dex_idx = (self.dex_idx + 1) % n;
                } else {
                    self.dex_idx = (self.dex_idx + n - 1) % n;
                }
            }
        }
    }

    fn handle_confirm(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                self.stage = Stage::Create;
                self.pending_plan = None;
                KeyOutcome::Consumed
            }
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(plan) = self.pending_plan.take() {
                    match add_plan(&self.profile_dir, plan) {
                        Ok(_) => {
                            self.stage = Stage::List;
                            self.reload();
                            self.status = "Plan saved".into();
                        }
                        Err(e) => {
                            self.status = e.to_string();
                            self.stage = Stage::Create;
                        }
                    }
                }
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::NotHandled,
        }
    }

    fn draft_venue(&self) -> Result<DcaVenue, String> {
        match self.route_kind {
            RouteKind::Agg => {
                let name = agg_picker_labels()
                    .get(self.agg_idx)
                    .copied()
                    .unwrap_or("auto")
                    .to_string();
                Ok(DcaVenue::Agg { name })
            }
            RouteKind::Dex => {
                let slugs = dex_picker_slugs(self.chain_id, DexProtocol::V2);
                let name = slugs
                    .get(self.dex_idx)
                    .copied()
                    .ok_or_else(|| format!("no V2 DEX routers on chain {}", self.chain_id))?
                    .to_string();
                Ok(DcaVenue::Dex {
                    name,
                    protocol: "v2".into(),
                })
            }
        }
    }

    fn draft_plan(
        &self,
        native_decimals: u8,
        account: Option<alloy::primitives::Address>,
    ) -> Result<DcaPlan, String> {
        let account =
            account.ok_or_else(|| "unlock a wallet before creating a DCA plan".to_string())?;
        let _ = parse_token_address(self.token_out.value(), "Token out")?;
        let amount_raw = self.amount.value().trim();
        let wei = if amount_raw.chars().all(|c| c.is_ascii_digit()) && amount_raw.len() >= 15 {
            amount_raw.to_string()
        } else {
            parse_native_amount(amount_raw, native_decimals)
                .map_err(|e: WalletError| e.user_message())?
        };
        let interval = INTERVAL_PRESETS
            .get(self.interval_idx)
            .map(|(s, _)| *s)
            .unwrap_or(MIN_INTERVAL_SECS);
        let max_slices: u32 = self
            .max_slices
            .value()
            .trim()
            .parse()
            .map_err(|_| "max slices: need a positive integer".to_string())?;
        let venue = self.draft_venue()?;
        venue.validate_for_chain(self.chain_id)?;
        build_plan(
            self.token_out.value(),
            &wei,
            interval,
            venue,
            100,
            max_slices,
            self.dry_run,
            self.chain_id,
            account,
        )
        .map_err(|e| e.to_string())
    }
}

fn focus_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(brand::accent_color())
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(brand::body_color())
    }
}

fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h{m}m")
        }
    } else {
        format!("{}d", secs / 86400)
    }
}

fn short_addr(addr: &str) -> String {
    let a = addr.trim();
    if a.len() <= 12 {
        a.to_string()
    } else {
        format!("{}…{}", &a[..6], &a[a.len().saturating_sub(4)..])
    }
}

fn short_hash(h: &str) -> String {
    let a = h.trim();
    if a.len() <= 14 {
        a.to_string()
    } else {
        format!("{}…{}", &a[..8], &a[a.len().saturating_sub(4)..])
    }
}

fn format_slice_human(wei: &str) -> String {
    let formatted = vaughan_core::core::format_base_units(wei, 18);
    format!("{formatted} native")
}

fn format_budget_human(p: &DcaPlan) -> String {
    match p.budget_wei() {
        Ok(b) => format!(
            "{} native",
            vaughan_core::core::format_base_units(&b.to_string(), 18)
        ),
        Err(_) => format!("{}×{}", p.slice_amount_wei, p.max_slices),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}
