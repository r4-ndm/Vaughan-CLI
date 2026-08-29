//! Placeholder screen for footer chips that are not implemented yet.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::views::{body_areas, status_paragraph};

pub struct PlaceholderView {
    screen: Screen,
    title: String,
    blurb: String,
}

impl PlaceholderView {
    pub fn new(screen: Screen, title: impl Into<String>, blurb: impl Into<String>) -> Self {
        Self {
            screen,
            title: title.into(),
            blurb: blurb.into(),
        }
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState) {
        let [body, status] = body_areas(area);
        let inner = brand::render_faded_box(
            frame,
            body,
            Some(brand::fade_line(&format!(" {} ", self.title))),
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    self.blurb.clone(),
                    ratatui::style::Style::default()
                        .fg(brand::body_color())
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Coming soon — Esc home",
                    ratatui::style::Style::default().fg(brand::accent_color()),
                )),
            ]),
            inner,
        );
        frame.render_widget(status_paragraph(""), status);
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        true
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        _wallet: &mut WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
            _ => KeyOutcome::NotHandled,
        }
    }
}
