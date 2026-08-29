//! Unlock: decrypt the vault and enter the wallet.
//!
//! When more than one profile vault exists, a profile picker runs first —
//! the profile chooses the agent mode (`default` = adviser; `sentient` =
//! agent auto-exec under policy + circuit breakers). The mode is locked in
//! at unlock and immutable for the session (FR-5.1).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};
use ratatui::Frame;
use tokio::runtime::Handle;
use vaughan_core::core::{
    is_sentient_profile, tui_mode_for_profile, ProfileMeta, StateManager, WalletState,
};
use vaughan_provider::{EventBus, ProviderEvent};

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::views::{render_labeled_input, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Pick the profile vault to unlock; the profile sets the agent mode.
    PickProfile,
    Password,
}

pub struct UnlockView {
    input: Input,
    status: String,
    profiles: Vec<ProfileMeta>,
    selected: usize,
    stage: Stage,
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
        }
    }
}

impl UnlockView {
    /// Build the unlock view around `current_profile` (the loaded wallet).
    ///
    /// The picker stage is skipped when only one profile exists — a single
    /// vault keeps the classic password-only screen.
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
        let selected = profiles
            .iter()
            .position(|p| p.name == current_profile)
            .unwrap_or(0);
        let stage = if profiles.len() > 1 {
            Stage::PickProfile
        } else {
            Stage::Password
        };
        Self {
            input: Input::new(true, "password"),
            status: String::new(),
            profiles,
            selected,
            stage,
        }
    }

    fn picker_enabled(&self) -> bool {
        self.profiles.len() > 1
    }

    fn mode_badge(meta: &ProfileMeta) -> &'static str {
        if meta.is_sentient {
            "Sentient — agent auto-exec"
        } else {
            "Advisor — you approve"
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
            Stage::PickProfile => {
                // +2: the faded box's top/bottom borders eat two rows.
                let list_h = self.profiles.len() as u16 + 2;
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
            Stage::PickProfile => self.render_picker(frame, body),
            Stage::Password => self.render_password(frame, body, wallet, policy.as_deref()),
        }
    }

    fn render_picker(&self, frame: &mut Frame, area: Rect) {
        let list_h = self.profiles.len() as u16 + 2;
        let [list_area, hint_area, status_area] = Layout::vertical([
            Constraint::Length(list_h),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let items: Vec<ListItem> = self
            .profiles
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let new_tag = if p.initialized {
                    ""
                } else {
                    "  (new — vault created next)"
                };
                let label = format!("  {:<12} {}{}", p.name, Self::mode_badge(p), new_tag);
                let style = if i == self.selected {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else if p.is_sentient {
                    Style::default().fg(Color::Magenta)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(Span::styled(label, style)))
            })
            .collect();
        let inner = brand::render_faded_box(
            frame,
            list_area,
            Some(brand::fade_line(" Profile — picks agent mode ")),
        );
        frame.render_widget(List::new(items), inner);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "↑↓ select · Enter continue",
                Style::default().fg(Color::DarkGray),
            ))),
            hint_area,
        );
        frame.render_widget(status_paragraph(&self.status), status_area);
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
        let mut profile_line = format!("Profile: {profile}");
        if is_sentient_profile(profile) {
            profile_line.push_str("  (Sentient — agent auto-exec under policy)");
        }
        if self.picker_enabled() {
            profile_line.push_str("  · Esc switch");
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
        render_labeled_input(frame, field, "Password", &self.input, true);
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        _handle: &Handle,
        events: &EventBus,
    ) -> KeyOutcome {
        match self.stage {
            Stage::PickProfile => self.handle_picker_key(key),
            Stage::Password => self.handle_password_key(key, wallet, events),
        }
    }

    fn handle_picker_key(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                if !self.profiles.is_empty() {
                    self.selected = (self.selected + 1).min(self.profiles.len() - 1);
                }
                KeyOutcome::Consumed
            }
            KeyCode::Enter => {
                let Some(meta) = self.profiles.get(self.selected) else {
                    return KeyOutcome::Consumed;
                };
                let name = meta.name.clone();
                let initialized = meta.initialized;
                self.input.set_value("");
                self.status.clear();
                if initialized {
                    self.stage = Stage::Password;
                }
                // Uninitialized profiles route to onboarding after the reload.
                KeyOutcome::SwitchProfile(name)
            }
            _ => KeyOutcome::NotHandled,
        }
    }

    fn handle_password_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        events: &EventBus,
    ) -> KeyOutcome {
        if key.code == KeyCode::Esc && self.picker_enabled() {
            self.stage = Stage::PickProfile;
            self.status.clear();
            return KeyOutcome::Consumed;
        }
        match self.input.handle_key(key) {
            InputAction::Ignored => KeyOutcome::NotHandled,
            InputAction::Submitted => {
                let password = self.input.take_secret();
                match wallet.unlock(&password) {
                    Ok(()) => {
                        // Mode follows the profile and locks for the session
                        // (FR-5.1): sentient profiles auto-exec under policy.
                        wallet.set_operating_mode(tui_mode_for_profile(wallet.profile_name()));
                        if let Ok(address) = wallet.active_address() {
                            events
                                .publish(ProviderEvent::AccountsChanged(vec![address.to_string()]));
                        }
                        self.status.clear();
                        KeyOutcome::Navigate(Screen::Dashboard)
                    }
                    Err(e) => {
                        self.status = e.user_message();
                        KeyOutcome::Consumed
                    }
                }
            }
            InputAction::Consumed => KeyOutcome::Consumed,
        }
    }
}
