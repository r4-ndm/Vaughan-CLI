//! View layer: shared chrome plus per-screen views.
//!
//! Layout: logo → blank → address (orange mid under AUGHA) → blank →
//! three status boxes (F1 network · F2 coin · F3 account) → view body → action footer.
//!
//! F1–F3: press the key to focus, ↑/↓ to preview, Enter to set, Esc to cancel.

pub mod aa_send;
pub mod ag;
pub mod agent;
pub mod agent_setup;
pub mod approve;
pub mod assets;
pub mod browser;
pub mod dapps;
pub mod dashboard;
pub mod dex;
pub mod dex_calldata;
pub mod keys;
pub mod onboarding;
pub mod placeholder;
pub mod receive;
pub mod send;
pub mod settings;
pub mod unlock;

pub use aa_send::AaSendView;
pub use ag::AgView;
pub use agent::AgentView;
pub use agent_setup::AgentSetupView;
pub use approve::ApproveView;
pub use assets::AssetsView;
pub use browser::BrowserView;
pub use dapps::DappsView;
pub use dashboard::DashboardView;
pub use dex::DexView;
pub use keys::KeysView;
pub use onboarding::OnboardingView;
pub use placeholder::PlaceholderView;
pub use receive::ReceiveView;
pub use send::SendView;
pub use settings::SettingsView;
pub use unlock::UnlockView;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

use crate::app::App;
use crate::brand;
use crate::input::Input;
use crate::jobs::{spinner_frame, ChromeFocus};

/// Render the full screen: wordmark, address, need-to-know status, body, footer.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let unlocked = app.try_wallet().is_some_and(|w| w.is_unlocked());
    // Status boxes + three rows of footer key chips when unlocked (no row gap).
    let status_h = 3u16;
    let footer_h = if unlocked { 9 } else { 3 };

    let [logo, _gap_above, addr_row, _gap_below, status_bar, body, footer] = Layout::vertical([
        Constraint::Length(1), // VAUGHAN banner
        Constraint::Length(1), // blank above address
        Constraint::Length(1), // colour-coded address
        Constraint::Length(1), // blank below address
        Constraint::Length(status_h),
        Constraint::Min(0),
        Constraint::Length(footer_h),
    ])
    .areas(area);

    frame.render_widget(Paragraph::new(brand::logo_banner(logo.width)), logo);
    render_address_under_augha(frame, addr_row, app);
    render_status_strip(frame, status_bar, app, unlocked);
    if unlocked {
        render_action_footer(frame, footer, app);
    } else {
        let [quit_box] = Layout::horizontal([Constraint::Percentage(100)]).areas(footer);
        // Centre a single quit chip.
        let w = quit_box.width.min(16);
        let x = quit_box.x + quit_box.width.saturating_sub(w) / 2;
        let chip = Rect {
            x,
            y: quit_box.y,
            width: w,
            height: quit_box.height.min(3),
        };
        render_key_chip(frame, chip, "x", "Quit");
    }
    app.render_body(frame, body);
    if app.quit_confirm().is_some() {
        render_quit_confirm(frame, area, app.quit_confirm() == Some(true));
    }
}

/// Colour-coded wallet address with orange mid-segment under `AUGHA` in the wordmark.
fn render_address_under_augha(frame: &mut Frame, area: Rect, app: &App) {
    let address = chrome_address(app);
    frame.render_widget(
        Paragraph::new(brand::colored_address_under_augha(&address, area.width)),
        area,
    );
}

fn render_status_strip(frame: &mut Frame, area: Rect, app: &App, unlocked: bool) {
    let chrome = app.chrome();
    let [net_area, token_area, acct_area] = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .spacing(1)
    .areas(area);

    if !unlocked {
        render_stat_box(frame, net_area, " F1 ", "—", false);
        render_stat_box(frame, token_area, " F2 ", "—", false);
        render_stat_box(frame, acct_area, " F3 ", "—", false);
        return;
    }

    let (net_name, testnet, native_sym) = {
        let pending_net = chrome
            .focus
            .eq(&ChromeFocus::Network)
            .then_some(chrome.pending_network_idx)
            .flatten();
        app.try_wallet()
            .map(|w| {
                let nets = w.networks().networks();
                let n = pending_net
                    .and_then(|i| nets.get(i))
                    .unwrap_or_else(|| w.networks().active());
                let test = if n.is_testnet { " · testnet" } else { "" };
                (n.name.clone(), test.to_string(), n.native_symbol.clone())
            })
            .unwrap_or_else(|| ("—".into(), String::new(), "ETH".into()))
    };

    let network_value = format!("{net_name}{testnet}");

    let asset_idx = if chrome.focus == ChromeFocus::Asset {
        chrome.pending_asset_idx.unwrap_or(chrome.asset_idx)
    } else {
        chrome.asset_idx
    };

    let token_value = if chrome.assets_loading && chrome.assets.is_empty() {
        format!("{} …", spinner_frame(app.tick()))
    } else if let Some(b) = chrome.assets.get(asset_idx) {
        format!("{} {}", b.formatted, b.token.symbol)
    } else if chrome.loading && chrome.balance.is_none() {
        format!("{} …", spinner_frame(app.tick()))
    } else {
        match &chrome.balance {
            Some(b) => format!("{} {}", b.formatted, b.token.symbol),
            None => format!("— {native_sym}"),
        }
    };

    let account_value = if chrome.focus == ChromeFocus::Account {
        chrome
            .pending_account_index
            .and_then(|idx| app.try_wallet().and_then(|w| w.account_label(idx).ok()))
            .unwrap_or_else(|| "—".into())
    } else {
        app.try_wallet()
            .and_then(|w| w.active_account_label().ok().map(str::to_string))
            .unwrap_or_else(|| "—".into())
    };

    render_stat_box(
        frame,
        net_area,
        " F1 ",
        &network_value,
        chrome.focus == ChromeFocus::Network,
    );
    render_stat_box(
        frame,
        token_area,
        " F2 ",
        &token_value,
        chrome.focus == ChromeFocus::Asset,
    );
    render_stat_box(
        frame,
        acct_area,
        " F3 ",
        &account_value,
        chrome.focus == ChromeFocus::Account,
    );
}

/// One faded square panel: themed title + centred value (accent ink).
/// When `focused`, title + value use reverse video so F1/F2/F3 are obvious.
fn render_stat_box(frame: &mut Frame, area: Rect, title: &str, value: &str, focused: bool) {
    let title_line = if focused {
        Line::from(Span::styled(
            title.to_string(),
            Style::default()
                .fg(brand::accent_color())
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        ))
    } else {
        brand::fade_line(title)
    };
    let inner = brand::render_faded_box(frame, area, Some(title_line));
    let value_style = if focused {
        Style::default()
            .fg(brand::accent_color())
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default()
            .fg(brand::accent_color())
            .add_modifier(Modifier::BOLD)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(value.to_string(), value_style)))
            .alignment(ratatui::layout::Alignment::Center),
        inner,
    );
}

fn render_action_footer(frame: &mut Frame, area: Rect, _app: &App) {
    // No `s Send` chip — Send stays available via global `s`.
    // Theme (`t`) is intentionally omitted from the footer.
    // 21 chips → three even rows of 7 (no vertical gap between rows).
    let keys: &[(&str, &str)] = &[
        ("v", "Recv"),
        ("a", "Assets"),
        ("b", "Batch"),
        ("w", "Dapps"),
        ("c", "Browse"),
        ("d", "Dex"),
        ("g", "Ag"),
        ("h", "Home"),
        ("n", "Net"),
        ("i", "Settings"),
        ("k", "Keys"),
        ("e", "NFT"),
        ("f", "Bridge"),
        ("j", "Stake"),
        ("m", "Hist"),
        ("q", "Agent"),
        ("r", "Refresh"),
        ("l", "Lock"),
        ("tab", "Next"),
        ("esc", "Back"),
        ("x", "Quit"),
    ];

    let n = keys.len();
    debug_assert_eq!(n, 21);
    let row_n = n / 3;
    let [row1, row2, row3] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
    ])
    .spacing(0)
    .areas(area);

    render_chip_row(frame, row1, &keys[..row_n]);
    render_chip_row(frame, row2, &keys[row_n..row_n * 2]);
    render_chip_row(frame, row3, &keys[row_n * 2..]);
}

/// Evenly spaced faded boxes, one per key chip.
fn render_chip_row(frame: &mut Frame, area: Rect, chips: &[(&str, &str)]) {
    if chips.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }
    let n = chips.len() as u32;
    let chunks = Layout::horizontal((0..n).map(|_| Constraint::Ratio(1, n)).collect::<Vec<_>>())
        .spacing(1)
        .split(area);
    for (cell, (k, label)) in chunks.iter().zip(chips.iter()) {
        render_key_chip(frame, *cell, k, label);
    }
}

fn render_key_chip(frame: &mut Frame, area: Rect, key: &str, label: &str) {
    let inner = brand::render_faded_box_with(frame, area, None, brand::FadePalette::Footer);
    let line = Line::from(vec![
        Span::styled(
            key.to_string(),
            Style::default()
                .fg(brand::accent_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(brand::body_color())
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(line).alignment(ratatui::layout::Alignment::Center),
        inner,
    );
}

/// Modal: "Are you sure you want to quit?" — Yes is the default (Enter quits).
fn render_quit_confirm(frame: &mut Frame, area: Rect, yes_selected: bool) {
    let width = 44u16.min(area.width.saturating_sub(4));
    let height = 7u16.min(area.height.saturating_sub(2));
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    let inner = brand::render_faded_box(frame, popup, None);
    let yes_style = if yes_selected {
        Style::default()
            .fg(brand::accent_color())
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(brand::body_color())
    };
    let no_style = if !yes_selected {
        Style::default()
            .fg(brand::accent_color())
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(brand::body_color())
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Are you sure you want to quit?",
            Style::default()
                .fg(brand::accent_color())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Yes  ", yes_style),
            Span::raw("   "),
            Span::styled("  No  ", no_style),
        ]),
        Line::from(Span::styled(
            "Enter confirm · Esc cancel · ←/→",
            Style::default().fg(brand::body_color()),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center),
        inner,
    );
}

/// Active account for the shared chrome strip (every screen).
fn chrome_address(app: &App) -> String {
    match app.try_wallet() {
        Some(wallet) if wallet.is_unlocked() => wallet
            .active_address()
            .map(str::to_string)
            .unwrap_or_else(|_| "(no account)".into()),
        Some(wallet) if wallet.is_initialized() => "(locked)".into(),
        Some(_) => "(create or restore a wallet)".into(),
        None => "(busy)".into(),
    }
}

/// Keep a full checksum address when possible; middle-ellipsis on narrow terminals.
#[cfg(test)]
fn fit_address(address: &str, width: u16) -> String {
    // " Address: " prefix is 10 columns.
    let budget = width.saturating_sub(10) as usize;
    fit_raw(address, budget)
}

#[cfg(test)]
fn fit_raw(address: &str, budget: usize) -> String {
    if budget == 0 {
        return String::new();
    }
    let chars: Vec<char> = address.chars().collect();
    if chars.len() <= budget {
        return address.to_string();
    }
    if budget <= 5 {
        return chars.into_iter().take(budget).collect();
    }
    let keep = budget.saturating_sub(1) / 2;
    let right = budget.saturating_sub(keep + 1);
    let mut out: String = chars.iter().take(keep).collect();
    out.push('…');
    out.extend(
        chars
            .iter()
            .rev()
            .take(right)
            .collect::<Vec<_>>()
            .into_iter()
            .rev(),
    );
    out
}

/// Render a labelled text input inside a faded square box (yellow title when focused).
pub(crate) fn render_labeled_input(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    input: &Input,
    focused: bool,
) {
    let title_text = format!(" {label} ");
    let title = if focused {
        brand::focus_title(&title_text)
    } else {
        brand::fade_line(&title_text)
    };
    let inner = brand::render_faded_box(frame, area, Some(title));
    let mut line = Line::from(Span::raw(format!("{label}: ")));
    line.extend(input.line());
    frame.render_widget(Paragraph::new(line), inner);
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

#[cfg(test)]
mod tests {
    use super::{fit_address, fit_raw};

    #[test]
    fn fit_address_keeps_full_when_wide() {
        let addr = "0x1234567890abcdef1234567890abcdef12345678";
        assert_eq!(fit_address(addr, 80), addr);
    }

    #[test]
    fn fit_address_ellipsizes_when_narrow() {
        let addr = "0x1234567890abcdef1234567890abcdef12345678";
        let fitted = fit_address(addr, 24); // budget 14
        assert!(fitted.contains('…'), "{fitted}");
        assert!(fitted.starts_with("0x"), "{fitted}");
        assert!(fitted.chars().count() <= 14, "{fitted}");
    }

    #[test]
    fn fit_raw_ellipsizes() {
        let addr = "0x1234567890abcdef1234567890abcdef12345678";
        let fitted = fit_raw(addr, 14);
        assert!(fitted.contains('…'), "{fitted}");
        assert!(fitted.chars().count() <= 14, "{fitted}");
    }
}
