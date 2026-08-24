//! Unlock: decrypt the vault and enter the wallet.

use crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use tokio::runtime::Handle;
use vaughan_core::core::{OperatingMode, WalletState};
use vaughan_provider::{EventBus, ProviderEvent};

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::views::{render_labeled_input, status_paragraph};

pub struct UnlockView {
    input: Input,
    status: String,
}

impl Default for UnlockView {
    fn default() -> Self {
        Self {
            input: Input::new(true, "password"),
            status: String::new(),
        }
    }
}

impl UnlockView {
    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState) {
        // Logo + password only — fixed-height input so the box does not stretch.
        let art = brand::logo_art_lines(area.width);
        let art_h = art.len() as u16;
        let gap = 1u16;
        let input_h = 3u16;
        let status_h = 1u16;
        let block = art_h.saturating_add(gap + input_h + status_h);
        let [_, mid, _] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(block.min(area.height)),
            Constraint::Min(0),
        ])
        .areas(area);
        let [art_area, _, input_area, status_area] = Layout::vertical([
            Constraint::Length(art_h),
            Constraint::Length(gap),
            Constraint::Length(input_h),
            Constraint::Length(status_h),
        ])
        .areas(mid);
        frame.render_widget(Paragraph::new(art), art_area);
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
        match self.input.handle_key(key) {
            InputAction::Ignored => KeyOutcome::NotHandled,
            InputAction::Submitted => {
                let password = self.input.take_secret();
                match wallet.unlock(&password) {
                    Ok(()) => {
                        wallet.set_operating_mode(OperatingMode::HumanOnly);
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
