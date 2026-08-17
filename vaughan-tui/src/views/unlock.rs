//! Unlock: decrypt the vault with the user's password.

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;

use crate::app::Screen;
use crate::input::Input;
use crate::views::{body_areas, labeled_input, status_paragraph};

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
        let [content, status_area] = body_areas(area);
        frame.render_widget(labeled_input("Password", &self.input, true), content);
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        _handle: &Handle,
    ) -> Option<Screen> {
        if self.input.handle_key(key) {
            let password = self.input.take_secret();
            match wallet.unlock(&password) {
                Ok(()) => return Some(Screen::Dashboard),
                Err(e) => self.status = e.user_message(),
            }
        }
        None
    }
}
