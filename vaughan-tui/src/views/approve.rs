//! Approve: the full-screen prompt shown when a dApp requests a sign/send.
//!
//! This view is render-only. It displays exactly what will be signed (method,
//! origin, recipient, value, chain, data) and tells the user which keys
//! approve (`y`/Enter) or deny (`n`/Esc). The decision and the actual signing
//! are handled by [`crate::app::App`], which owns the pending request's reply
//! channel and the wallet state.

use crossterm::event::KeyEvent;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::brand;

pub struct ApproveView {
    title: String,
    origin: Option<String>,
    details: Vec<String>,
}

impl ApproveView {
    pub fn new(title: String, origin: Option<String>, details: Vec<String>) -> Self {
        Self {
            title,
            origin,
            details,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState) {
        let origin = self.origin.as_deref().unwrap_or("(no origin)");
        let mut text = vec![
            Line::from(Span::styled(
                self.title.clone(),
                Style::default().fg(Color::Yellow),
            )),
            Line::from(format!("Origin:  {origin}")),
            Line::from(""),
        ];
        for detail in &self.details {
            text.push(Line::from(detail.clone()));
        }
        text.push(Line::from(""));
        text.push(Line::from("y / Enter — approve     n / Esc — deny"));

        let inner =
            brand::render_faded_box(frame, area, Some(brand::fade_line(" Approve request ")));
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
    }

    pub fn handle_key(
        &mut self,
        _key: KeyEvent,
        _wallet: &mut WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        // The decision is handled by `App` (which owns the reply channel and
        // returns the user to their previous screen); this view only renders.
        KeyOutcome::NotHandled
    }
}
