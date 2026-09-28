//! Optional web dApps: whitelisted origins for the EIP-1193 bridge.
//!
//! Not the default Browserless Pulse path — use Ag / Dex / LP / MCP first.
//! Contract browser (power tool) lives under Settings → c.
//! Enter opens **VB** when installed, else Freedom (dev fallback; integration
//! parked until upstream PR #195) — never the system browser.

use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::core::vb_browser::delete_vb_saved_profile;
use vaughan_core::core::{trusted_dapp_allow_hosts, TrustedDapp, WalletState};
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::brand;
use crate::freedom;
use crate::input::{Input, InputAction};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    List,
    Add,
}

pub struct DappsView {
    stage: Stage,
    selected: usize,
    /// First visible list row; nudged in render so `selected` stays on screen.
    scroll: Cell<usize>,
    name: Input,
    url: Input,
    focus: usize,
    status: String,
}

impl Default for DappsView {
    fn default() -> Self {
        Self {
            stage: Stage::List,
            selected: 0,
            scroll: Cell::new(0),
            name: Input::new(false, "PulseX"),
            url: Input::new(false, "https://…"),
            focus: 0,
            status: String::new(),
        }
    }
}

impl DappsView {
    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState, bridge_line: &str) {
        let [content, status_area] = body_areas(area);
        let dapps = wallet.trusted_dapps();

        match self.stage {
            Stage::List => {
                let inner = brand::render_faded_box(
                    frame,
                    content,
                    Some(brand::fade_line(
                        " Optional web (↑↓ · Enter → browser · s save site data · a add · d delete · Esc) ",
                    )),
                );
                let bridge_h = u16::from(!bridge_line.is_empty());
                let [bridge_a, list_a] =
                    Layout::vertical([Constraint::Length(bridge_h), Constraint::Min(1)])
                        .areas(inner);
                if bridge_h > 0 {
                    frame.render_widget(Paragraph::new(bridge_line.to_string()), bridge_a);
                }
                if dapps.is_empty() {
                    frame.render_widget(
                        Paragraph::new("  No sites yet — press a to add one (optional web path)."),
                        list_a,
                    );
                } else {
                    let saved: Vec<bool> = dapps
                        .iter()
                        .map(|d| wallet.vb_keeps_profile(&d.url))
                        .collect();
                    self.render_dapp_list(frame, list_a, &dapps, &saved);
                }
            }
            Stage::Add => {
                let [msg, name_a, url_a] = Layout::vertical([
                    Constraint::Min(2),
                    Constraint::Length(3),
                    Constraint::Length(3),
                ])
                .areas(content);
                let msg_inner =
                    brand::render_faded_box(frame, msg, Some(brand::fade_line(" Add dApp ")));
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from("Add a whitelisted dApp"),
                        Line::from("Tab switches fields · Enter saves · Esc cancels"),
                        Line::from(
                            "URLs open in VB when installed; Freedom only as dev fallback (parked until PR #195).",
                        ),
                    ])
                    .wrap(Wrap { trim: false }),
                    msg_inner,
                );
                render_labeled_input(frame, name_a, "Name", &self.name, self.focus == 0);
                render_labeled_input(frame, url_a, "URL", &self.url, self.focus == 1);
            }
        }
        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    fn render_dapp_list(
        &self,
        frame: &mut Frame,
        area: Rect,
        dapps: &[TrustedDapp],
        saved: &[bool],
    ) {
        let rows = usize::from(area.height.max(1));
        let top = scroll_top(self.scroll.get(), self.selected, rows, dapps.len());
        self.scroll.set(top);
        let overflow = dapps.len() > rows;
        // Keep the rightmost column for the scroll bar when it is shown.
        let row_w = area.width.saturating_sub(u16::from(overflow));
        let buf = frame.buffer_mut();
        for (slot, (i, d)) in dapps.iter().enumerate().skip(top).take(rows).enumerate() {
            let cell = Rect::new(area.x, area.y.saturating_add(slot as u16), row_w, 1);
            let keeps = saved.get(i).copied().unwrap_or(false);
            render_dapp_cell(buf, cell, d, i == self.selected, keeps);
        }
        if overflow {
            let mut state = ScrollbarState::new(dapps.len() - rows + 1)
                .viewport_content_length(rows)
                .position(top);
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .end_symbol(None),
                area,
                &mut state,
            );
        }
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        matches!(self.stage, Stage::List)
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        _handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        match self.stage {
            Stage::List => match key.code {
                KeyCode::Esc => KeyOutcome::Back,
                KeyCode::Char('a') => {
                    self.stage = Stage::Add;
                    self.focus = 0;
                    self.status.clear();
                    KeyOutcome::Consumed
                }
                KeyCode::Up => {
                    self.selected = self.selected.saturating_sub(1);
                    KeyOutcome::Consumed
                }
                KeyCode::Down => {
                    let len = wallet.trusted_dapps().len();
                    if len > 0 {
                        self.selected = (self.selected + 1).min(len - 1);
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    let dapps = wallet.trusted_dapps();
                    if let Some(TrustedDapp { url, .. }) = dapps.get(self.selected) {
                        let allow_hosts = trusted_dapp_allow_hosts(&dapps);
                        match freedom::open_dapp_url(
                            url,
                            &allow_hosts,
                            wallet.agent_browser_control(),
                            wallet.vb_keeps_profile(url),
                        ) {
                            Ok(msg) => self.status = msg,
                            Err(e) => self.status = e,
                        }
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::Char('s') => {
                    let dapps = wallet.trusted_dapps();
                    if let Some(TrustedDapp { name, url, .. }) = dapps.get(self.selected) {
                        let keep = !wallet.vb_keeps_profile(url);
                        self.status = match wallet.set_vb_keeps_profile(url, keep) {
                            Ok(()) if keep => format!(
                                "{name}: site data (settings, logins) kept between VB launches."
                            ),
                            Ok(()) => match delete_vb_saved_profile(url) {
                                Ok(true) => format!("{name}: saved site data deleted."),
                                Ok(false) => format!("{name}: throwaway profile each launch."),
                                Err(e) => e.user_message(),
                            },
                            Err(e) => e.user_message(),
                        };
                    }
                    KeyOutcome::Consumed
                }
                KeyCode::Char('d') => {
                    let dapps = wallet.trusted_dapps();
                    if let Some(TrustedDapp { url, .. }) = dapps.get(self.selected).cloned() {
                        match wallet.remove_trusted_dapp(&url) {
                            Ok(()) => {
                                let _ = delete_vb_saved_profile(&url);
                                self.status = "Removed dApp.".into();
                                self.selected = self.selected.saturating_sub(1);
                            }
                            Err(e) => self.status = e.user_message(),
                        }
                    }
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::NotHandled,
            },
            Stage::Add => {
                if key.code == KeyCode::Esc {
                    self.stage = Stage::List;
                    return KeyOutcome::Consumed;
                }
                if key.code == KeyCode::Tab {
                    self.focus = 1 - self.focus;
                    return KeyOutcome::Consumed;
                }
                let action = if self.focus == 0 {
                    self.name.handle_key(key)
                } else {
                    self.url.handle_key(key)
                };
                match action {
                    InputAction::Ignored => KeyOutcome::NotHandled,
                    InputAction::Consumed => KeyOutcome::Consumed,
                    InputAction::Submitted if self.focus == 0 => {
                        self.focus = 1;
                        KeyOutcome::Consumed
                    }
                    InputAction::Submitted => {
                        match wallet.add_trusted_dapp(self.name.value(), self.url.value()) {
                            Ok(d) => {
                                self.status = format!(
                                    "Added {} — Enter opens VB or Freedom fallback.",
                                    d.name
                                );
                                self.name.set_value("");
                                self.url.set_value("");
                                self.stage = Stage::List;
                            }
                            Err(e) => self.status = e.user_message(),
                        }
                        KeyOutcome::Consumed
                    }
                }
            }
        }
    }
}

/// First visible row: move `top` only as far as needed to keep `selected` in a
/// `rows`-tall window, and never leave blank rows below the last entry.
fn scroll_top(top: usize, selected: usize, rows: usize, len: usize) -> usize {
    let top = if selected < top {
        selected
    } else if selected >= top + rows {
        selected + 1 - rows
    } else {
        top
    };
    top.min(len.saturating_sub(rows))
}

/// One `> Name  host` entry (`· saved` when VB keeps its profile), clipped to `area.width`.
fn render_dapp_cell(buf: &mut Buffer, area: Rect, d: &TrustedDapp, selected: bool, saved: bool) {
    let mark = if selected { ">" } else { " " };
    let row_style = if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let url_style = if selected {
        row_style
    } else {
        Style::default().fg(Color::Cyan)
    };

    let prefix = format!("{mark} {}  ", d.name);
    let prefix_w = Line::from(prefix.as_str()).width() as u16;
    buf.set_stringn(area.x, area.y, &prefix, area.width as usize, row_style);

    if prefix_w < area.width {
        let url_x = area.x.saturating_add(prefix_w);
        let url_w = area.width.saturating_sub(prefix_w) as usize;
        // Host-only (no https://) so terminals do not auto-link / look "broken".
        let mut shown = freedom::display_host(&d.url);
        if saved {
            shown.push_str("  · saved");
        }
        buf.set_stringn(url_x, area.y, &shown, url_w, url_style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_stays_put_while_selection_visible() {
        assert_eq!(scroll_top(0, 5, 10, 27), 0);
        assert_eq!(scroll_top(4, 9, 10, 27), 4);
    }

    #[test]
    fn scroll_follows_selection_off_either_edge() {
        assert_eq!(scroll_top(0, 10, 10, 27), 1);
        assert_eq!(scroll_top(0, 26, 10, 27), 17);
        assert_eq!(scroll_top(12, 3, 10, 27), 3);
    }

    #[test]
    fn scroll_never_leaves_blank_tail() {
        assert_eq!(scroll_top(20, 20, 10, 27), 17);
        assert_eq!(scroll_top(5, 2, 10, 8), 0);
    }
}
