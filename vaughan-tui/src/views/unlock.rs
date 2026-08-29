//! Unlock: decrypt the vault and enter the wallet.
//!
//! The picker asks **Human or Sentient** first — whose seed backs the
//! session. A Human wallet then picks the operating mode: **Human only**
//! (no agent surface) or **Advisor** (agent proposes, human approves). The
//! Sentient wallet is always **Sentient** (agent auto-exec under policy +
//! circuit breakers) — auto-exec never runs on a human wallet's seed. The
//! mode is locked in at unlock and immutable for the session (FR-5.1).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};
use ratatui::Frame;
use tokio::runtime::Handle;
use vaughan_core::core::{
    is_sentient_profile, tui_mode_for_profile, OperatingMode, ProfileMeta, StateManager,
    WalletState, SENTIENT_PROFILE,
};
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::UiJob;
use crate::views::{render_labeled_input, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Human or Sentient — whose seed backs this session.
    PickRole,
    /// Which human wallet (only shown with several human profiles).
    PickWallet,
    /// Operating mode for the picked human wallet.
    PickMode,
    Password,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Human,
    Sentient,
}

/// Modes offered for a human wallet, in picker order.
const HUMAN_WALLET_MODES: [OperatingMode; 2] =
    [OperatingMode::HumanOnly, OperatingMode::AiAssisted];

pub struct UnlockView {
    input: Input,
    status: String,
    profiles: Vec<ProfileMeta>,
    /// Cursor into the current stage's rows.
    selected: usize,
    stage: Stage,
    /// Where Esc from the password stage returns to.
    password_back: Option<Stage>,
    role: Option<Role>,
    picked_profile: Option<String>,
    /// Mode chosen on the picker; `None` on the password-only path derives
    /// the mode from the profile name (single-vault / test UX).
    picked_mode: Option<OperatingMode>,
    /// True while the vault KDF runs off-thread (`UiJob::Unlock`); the view
    /// shows a spinner and swallows keys until the app reports the result.
    unlocking: bool,
    /// UI tick counter driving the spinner animation.
    tick: u64,
}

impl Default for UnlockView {
    /// Password-only unlock with no profile scan (tests; single-vault UX).
    fn default() -> Self {
        Self {
            input: Input::new(true, "password"),
            status: String::new(),
            profiles: Vec::new(),
            selected: 0,
            stage: Stage::Password,
            password_back: None,
            role: None,
            picked_profile: None,
            picked_mode: None,
            unlocking: false,
            tick: 0,
        }
    }
}

impl UnlockView {
    /// Build the unlock view around `current_profile` (the loaded wallet).
    pub fn new(current_profile: &str) -> Self {
        let mut profiles = StateManager::list_profiles();
        if !profiles.iter().any(|p| p.name == current_profile) {
            // Launch profile with no vault dir yet — list it so it is pickable.
            if let Ok(path) = StateManager::profile_path(current_profile) {
                profiles.push(ProfileMeta {
                    name: current_profile.to_string(),
                    initialized: false,
                    is_sentient: is_sentient_profile(current_profile),
                    path,
                });
            }
        }
        Self::with_profiles(profiles, current_profile)
    }

    /// Picker construction from an explicit profile list (tests inject fakes).
    pub fn with_profiles(profiles: Vec<ProfileMeta>, current_profile: &str) -> Self {
        let current_is_sentient = is_sentient_profile(current_profile);
        Self {
            input: Input::new(true, "password"),
            status: String::new(),
            selected: usize::from(current_is_sentient),
            stage: if profiles.is_empty() {
                Stage::Password
            } else {
                Stage::PickRole
            },
            password_back: None,
            role: None,
            picked_profile: None,
            picked_mode: None,
            unlocking: false,
            tick: 0,
            profiles,
        }
    }

    /// Surface an app-side error (e.g. a failed profile switch) on the status
    /// line; the chrome flash is not rendered on this screen.
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
    }

    /// Drive the unlock spinner; called once per UI tick while this view lives.
    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    /// The off-thread unlock finished — success is handled by the app
    /// (accounts applied, navigate away); this is the failure path.
    pub fn unlock_failed(&mut self, msg: impl Into<String>) {
        self.unlocking = false;
        self.status = msg.into();
    }

    fn human_profiles(&self) -> Vec<&ProfileMeta> {
        self.profiles.iter().filter(|p| !p.is_sentient).collect()
    }

    /// The sentient wallet on disk, preferring the canonical `sentient`
    /// name over the legacy `degen` one.
    fn sentient_meta(&self) -> Option<&ProfileMeta> {
        self.profiles
            .iter()
            .find(|p| p.name == SENTIENT_PROFILE)
            .or_else(|| self.profiles.iter().find(|p| p.is_sentient))
    }

    fn mode_badge(mode: OperatingMode) -> &'static str {
        match mode {
            OperatingMode::HumanOnly => "Human only — manual wallet, no agent",
            OperatingMode::AiAssisted => "Advisor — agent proposes, you approve",
            OperatingMode::SentientTrader => "Sentient — agent auto-exec under policy",
        }
    }

    fn row_count(&self) -> usize {
        match self.stage {
            Stage::PickRole => 2,
            Stage::PickWallet => self.human_profiles().len(),
            Stage::PickMode => HUMAN_WALLET_MODES.len(),
            Stage::Password => 0,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let art = brand::logo_art_lines(area.width);
        let art_h = art.len() as u16;
        let gap = 1u16;
        let input_h = 3u16;
        let status_h = 1u16;
        let policy = crate::sentient_mcp::sentient_policy_line(wallet);
        let info_h = if policy.is_some() { 2u16 } else { 1u16 };
        let block = match self.stage {
            Stage::Password => art_h.saturating_add(gap + info_h + input_h + status_h),
            Stage::PickRole | Stage::PickWallet | Stage::PickMode => {
                // +2: the faded box's top/bottom borders eat two rows.
                let list_h = self.row_count() as u16 + 2;
                art_h.saturating_add(gap + list_h + 1 + status_h)
            }
        };
        let [_, mid, _] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(block.min(area.height)),
            Constraint::Min(0),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(art),
            Rect {
                height: art_h,
                ..mid
            },
        );
        let body = Rect {
            y: mid.y + art_h + gap,
            height: mid.height.saturating_sub(art_h + gap),
            ..mid
        };
        match self.stage {
            Stage::PickRole => self.render_roles(frame, body),
            Stage::PickWallet => self.render_wallets(frame, body),
            Stage::PickMode => self.render_modes(frame, body),
            Stage::Password => self.render_password(frame, body, wallet, policy.as_deref()),
        }
    }

    fn render_list(
        &self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        labels: Vec<(String, Style)>,
        hint: &str,
    ) {
        let list_h = labels.len() as u16 + 2;
        let [list_area, hint_area, status_area] = Layout::vertical([
            Constraint::Length(list_h),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let items: Vec<ListItem> = labels
            .into_iter()
            .map(|(label, style)| ListItem::new(Line::from(Span::styled(label, style))))
            .collect();
        let inner = brand::render_faded_box(frame, list_area, Some(brand::fade_line(title)));
        frame.render_widget(List::new(items), inner);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hint.to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            hint_area,
        );
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    fn render_roles(&self, frame: &mut Frame, area: Rect) {
        let human_new = self.human_profiles().is_empty();
        let sentient_new = self.sentient_meta().map(|m| !m.initialized).unwrap_or(true);
        let rows = [
            (Role::Human, "Human", human_new, Style::default()),
            (
                Role::Sentient,
                "Sentient",
                sentient_new,
                Style::default().fg(Color::Magenta),
            ),
        ];
        let labels = rows
            .iter()
            .enumerate()
            .map(|(i, (_, label, is_new, base))| {
                let tag = if *is_new {
                    "  (new — vault created next)"
                } else {
                    ""
                };
                let style = if i == self.selected {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    *base
                };
                (format!("  {label}{tag}"), style)
            })
            .collect();
        self.render_list(
            frame,
            area,
            " Wallet ",
            labels,
            "↑↓ select · Enter continue",
        );
    }

    fn render_wallets(&self, frame: &mut Frame, area: Rect) {
        let labels = self
            .human_profiles()
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let new_tag = if p.initialized {
                    ""
                } else {
                    "  (new — vault created next)"
                };
                let style = if i == self.selected {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default()
                };
                (format!("  {:<12} your wallet{new_tag}", p.name), style)
            })
            .collect();
        self.render_list(
            frame,
            area,
            " Human — which wallet ",
            labels,
            "↑↓ select · Enter continue · Esc back",
        );
    }

    fn render_modes(&self, frame: &mut Frame, area: Rect) {
        let labels = HUMAN_WALLET_MODES
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let label = format!("  {}", Self::mode_badge(*m));
                let style = if i == self.selected {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    match m {
                        OperatingMode::AiAssisted => Style::default().fg(Color::Green),
                        _ => Style::default(),
                    }
                };
                (label, style)
            })
            .collect();
        let wallet_name = self.picked_profile.as_deref().unwrap_or("default");
        let title = format!(" Mode for {wallet_name} — locked for the session ");
        self.render_list(
            frame,
            area,
            &title,
            labels,
            "↑↓ select · Enter continue · Esc back",
        );
    }

    fn render_password(
        &self,
        frame: &mut Frame,
        area: Rect,
        wallet: &WalletState,
        policy: Option<&str>,
    ) {
        let info_h = if policy.is_some() { 2u16 } else { 1u16 };
        let [profile_area, input_area, status_area] = Layout::vertical([
            Constraint::Length(info_h),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .areas(area);
        let profile = wallet.profile_name();
        let mode = self
            .picked_mode
            .unwrap_or_else(|| tui_mode_for_profile(profile));
        let mut profile_line = format!("Profile: {profile}  ({})", Self::mode_badge(mode));
        if self.password_back.is_some() && !self.unlocking {
            profile_line.push_str("  · Esc back");
        }
        let mut info_lines = vec![Line::from(Span::styled(
            profile_line,
            Style::default().fg(Color::DarkGray),
        ))];
        if let Some(policy) = policy {
            let style = if policy.contains("DISABLED") || policy.contains("WARN-ONLY") {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            info_lines.push(Line::from(Span::styled(format!("  {policy}"), style)));
        }
        frame.render_widget(Paragraph::new(info_lines), profile_area);
        // Keep the password field a comfortable width, centred.
        let field_w = input_area.width.min(56).max(24.min(input_area.width));
        let field_x = input_area.x + (input_area.width.saturating_sub(field_w)) / 2;
        let field = Rect {
            x: field_x,
            y: input_area.y,
            width: field_w,
            height: input_area.height,
        };
        if self.unlocking {
            // The password was moved into the unlock job; show a fixed mask
            // (not the real length) plus an animated spinner for the KDF wait.
            let inner = brand::render_faded_box(frame, field, Some(brand::fade_line(" Password ")));
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "••••••••",
                    Style::default().fg(Color::DarkGray),
                ))),
                inner,
            );
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(
                        "{} Unlocking — Argon2id key derivation…",
                        crate::jobs::spinner_frame(self.tick)
                    ),
                    Style::default().fg(Color::Yellow),
                ))),
                status_area,
            );
        } else {
            render_labeled_input(frame, field, "Password", &self.input, true);
            frame.render_widget(status_paragraph(&self.status), status_area);
        }
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        // While the KDF runs off-thread, swallow input (it can't be cancelled
        // mid-hash; Ctrl+C/Ctrl+Q still quit via the app's global handling).
        if self.unlocking {
            return KeyOutcome::Consumed;
        }
        match self.stage {
            Stage::PickRole => self.handle_role_key(key),
            Stage::PickWallet => self.handle_wallet_key(key),
            Stage::PickMode => self.handle_mode_key(key),
            Stage::Password => self.handle_password_key(key, wallet),
        }
    }

    fn move_cursor(&mut self, delta: i64) {
        let len = self.row_count();
        if len == 0 {
            return;
        }
        let max = len as i64 - 1;
        let next = (self.selected as i64 + delta).clamp(0, max);
        self.selected = next as usize;
    }

    fn handle_role_key(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Up => {
                self.move_cursor(-1);
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                self.move_cursor(1);
                KeyOutcome::Consumed
            }
            KeyCode::Enter => {
                let role = if self.selected == 0 {
                    Role::Human
                } else {
                    Role::Sentient
                };
                self.input.set_value("");
                self.status.clear();
                match role {
                    Role::Human => {
                        self.role = Some(Role::Human);
                        let humans = self.human_profiles();
                        if humans.len() > 1 {
                            self.selected = 0;
                            self.stage = Stage::PickWallet;
                        } else {
                            // One (or zero — created next) human wallet:
                            // straight to the mode choice.
                            let name = humans
                                .first()
                                .map(|p| p.name.clone())
                                .unwrap_or_else(|| "default".to_string());
                            self.picked_profile = Some(name);
                            self.selected = 1; // pre-select Advisor
                            self.stage = Stage::PickMode;
                        }
                        KeyOutcome::Consumed
                    }
                    Role::Sentient => {
                        // The sentient wallet has exactly one mode — skip
                        // the mode step entirely.
                        let (name, initialized) = self
                            .sentient_meta()
                            .map(|m| (m.name.clone(), m.initialized))
                            .unwrap_or_else(|| (SENTIENT_PROFILE.to_string(), false));
                        self.role = Some(Role::Sentient);
                        self.picked_profile = Some(name.clone());
                        self.picked_mode = Some(OperatingMode::SentientTrader);
                        self.password_back = Some(Stage::PickRole);
                        if initialized {
                            self.stage = Stage::Password;
                        }
                        KeyOutcome::SwitchProfile(name, OperatingMode::SentientTrader)
                    }
                }
            }
            _ => KeyOutcome::NotHandled,
        }
    }

    fn handle_wallet_key(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Up => {
                self.move_cursor(-1);
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                self.move_cursor(1);
                KeyOutcome::Consumed
            }
            KeyCode::Esc => {
                self.selected = usize::from(self.role == Some(Role::Sentient));
                self.stage = Stage::PickRole;
                KeyOutcome::Consumed
            }
            KeyCode::Enter => {
                let humans = self.human_profiles();
                let Some(meta) = humans.get(self.selected) else {
                    return KeyOutcome::Consumed;
                };
                self.picked_profile = Some(meta.name.clone());
                self.selected = 1; // pre-select Advisor
                self.stage = Stage::PickMode;
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::NotHandled,
        }
    }

    fn handle_mode_key(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Up => {
                self.move_cursor(-1);
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                self.move_cursor(1);
                KeyOutcome::Consumed
            }
            KeyCode::Esc => {
                if self.human_profiles().len() > 1 {
                    self.selected = 0;
                    self.stage = Stage::PickWallet;
                } else {
                    self.selected = 0;
                    self.stage = Stage::PickRole;
                }
                KeyOutcome::Consumed
            }
            KeyCode::Enter => {
                let Some(mode) = HUMAN_WALLET_MODES.get(self.selected).copied() else {
                    return KeyOutcome::Consumed;
                };
                let Some(name) = self.picked_profile.clone() else {
                    return KeyOutcome::Consumed;
                };
                let initialized = self
                    .profiles
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| p.initialized)
                    .unwrap_or(false);
                self.picked_mode = Some(mode);
                self.input.set_value("");
                self.status.clear();
                self.password_back = Some(Stage::PickMode);
                if initialized {
                    self.stage = Stage::Password;
                }
                // Uninitialized profiles route to onboarding after the reload.
                KeyOutcome::SwitchProfile(name, mode)
            }
            _ => KeyOutcome::NotHandled,
        }
    }

    fn handle_password_key(&mut self, key: KeyEvent, wallet: &mut WalletState) -> KeyOutcome {
        if key.code == KeyCode::Esc && self.password_back.is_some() {
            self.stage = self.password_back.unwrap_or(Stage::Password);
            self.status.clear();
            return KeyOutcome::Consumed;
        }
        match self.input.handle_key(key) {
            InputAction::Ignored => KeyOutcome::NotHandled,
            InputAction::Submitted => {
                let password = self.input.take_secret();
                // Mode locks for the session (FR-5.1): the picked row wins;
                // the password-only path derives it from the profile name.
                let mode = self
                    .picked_mode
                    .unwrap_or_else(|| tui_mode_for_profile(wallet.profile_name()));
                self.unlocking = true;
                self.status.clear();
                // Argon2id runs off the UI thread so the spinner can animate;
                // the app applies the result (accounts + mode) on completion.
                KeyOutcome::StartJob(UiJob::Unlock { password, mode })
            }
            InputAction::Consumed => KeyOutcome::Consumed,
        }
    }
}
