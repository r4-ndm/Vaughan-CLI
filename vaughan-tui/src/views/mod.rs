//! View layer: shared chrome plus per-screen views.

pub mod aa_send;
pub mod agent;
pub mod agent_setup;
pub mod approve;
pub mod assets;
pub mod browser;
pub mod dapps;
pub mod dashboard;
pub mod keys;
pub mod onboarding;
pub mod receive;
pub mod send;
pub mod settings;
pub mod unlock;

pub use aa_send::AaSendView;
pub use agent::AgentView;
pub use agent_setup::AgentSetupView;
pub use approve::ApproveView;
pub use assets::AssetsView;
pub use browser::BrowserView;
pub use dapps::DappsView;
pub use dashboard::DashboardView;
pub use keys::KeysView;
pub use onboarding::OnboardingView;
pub use receive::ReceiveView;
pub use send::SendView;
pub use settings::SettingsView;
pub use unlock::UnlockView;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::brand;
use crate::input::Input;

/// Render the full screen: ASCII wordmark, title bar, active view body, footer.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let [logo, title_bar, body, footer] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    frame.render_widget(Paragraph::new(brand::logo_lines()), logo);

    let status = if let Some(wallet) = app.try_wallet() {
        if wallet.is_unlocked() {
            "unlocked"
        } else if wallet.is_initialized() {
            "locked"
        } else {
            "new wallet"
        }
    } else {
        "busy"
    };
    let busy = if app.try_wallet().is_none() {
        format!(" {} ", crate::jobs::spinner_frame(app.tick()))
    } else {
        String::new()
    };
    frame.render_widget(
        Block::default()
            .title(format!(
                " Vaughan CLI — {} [{status}]{busy}",
                app.screen().title()
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
        title_bar,
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" q ", Style::default().bg(Color::Cyan).fg(Color::Black)),
            Span::raw(" quit   "),
            Span::styled(" tab ", Style::default().bg(Color::Cyan).fg(Color::Black)),
            Span::raw(" next   "),
            Span::styled(" esc ", Style::default().bg(Color::Cyan).fg(Color::Black)),
            Span::raw(" back "),
        ])),
        footer,
    );

    app.render_body(frame, body);
}

/// A labelled text-input widget (highlighted border when focused).
pub(crate) fn labeled_input(label: &str, input: &Input, focused: bool) -> Paragraph<'static> {
    let mut line = Line::from(Span::raw(format!("{label}: ")));
    line.extend(input.line());
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    Paragraph::new(line).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style),
    )
}

/// A status/error line rendered at the bottom of a view's body.
pub(crate) fn status_paragraph(status: &str) -> Paragraph<'static> {
    let style = if status.is_empty() {
        Style::default()
    } else {
        Style::default().fg(Color::Red)
    };
    Paragraph::new(Span::styled(
        if status.is_empty() {
            " ".to_string()
        } else {
            status.to_string()
        },
        style,
    ))
}

/// Split a view body into content + a status line.
pub(crate) fn body_areas(area: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area)
}
