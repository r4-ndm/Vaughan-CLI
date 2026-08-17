//! Settings: switch the active network.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;

use crate::app::Screen;
use crate::views::{body_areas, status_paragraph};

pub struct SettingsView {
    selected: usize,
    status: String,
}

impl SettingsView {
    pub fn new(selected: usize) -> Self {
        Self {
            selected,
            status: String::new(),
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let networks = wallet.networks();
        let active_id = networks.active_id();

        let items: Vec<ListItem> = networks
            .networks()
            .iter()
            .enumerate()
            .map(|(i, net)| {
                let mark = if net.id == active_id { " * " } else { "   " };
                let label = format!(
                    "{mark}{}  ({})  chain {}",
                    net.name, net.native_symbol, net.chain_id
                );
                let style = if i == self.selected {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(Span::styled(label, style)))
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Networks (up/down to move, Enter to switch) "),
        );
        frame.render_widget(list, content);
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        _handle: &Handle,
    ) -> Option<Screen> {
        let len = wallet.networks().networks().len();
        match key.code {
            KeyCode::Esc => Some(Screen::Dashboard),
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(len.saturating_sub(1));
                None
            }
            KeyCode::Enter => {
                if let Some(net) = wallet.networks().networks().get(self.selected) {
                    let id = net.id.clone();
                    let name = net.name.clone();
                    match wallet.set_active_network(&id) {
                        Ok(()) => self.status = format!("Switched to {name}."),
                        Err(e) => self.status = e.user_message(),
                    }
                }
                None
            }
            _ => None,
        }
    }
}
