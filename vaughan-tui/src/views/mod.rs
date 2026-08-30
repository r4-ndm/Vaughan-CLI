//! View layer: shared chrome plus per-screen views.
//!
//! Layout: logo → blank → address (orange mid under AUGHA) → blank →
//! three status boxes (F1 network · F2 coin · F3 account) → blank →
//! view body → action footer.
//!
//! F1–F3: press the key to focus, ↑/↓ to preview, Enter to set, Esc to cancel.
//! Home send body: F4 focuses recipient, F5 focuses amount. Dex / Ag / Bridge: F4 confirm or quote.

pub mod aa_send;
pub mod ag;
pub mod approvals;
pub mod approve;
pub mod assets;
pub mod bridge;
pub mod browser;
pub mod dapps;
pub mod dashboard;
pub mod dex;
pub mod dex_calldata;
pub mod history;
pub mod keys;
pub mod lp;
pub mod onboarding;
pub mod placeholder;
pub mod receive;
pub mod send;
pub mod settings;
pub mod swap_form;
pub mod unlock;
pub mod wrap;

pub use aa_send::AaSendView;
pub use ag::AgView;
pub use approvals::ApprovalsView;
pub use approve::ApproveView;
pub use assets::AssetsView;
pub use bridge::BridgeView;
pub use browser::BrowserView;
pub use dapps::DappsView;
pub use dashboard::DashboardView;
pub use dex::DexView;
pub use history::HistoryView;
pub use keys::KeysView;
pub use lp::LpView;
pub use onboarding::OnboardingView;
pub use placeholder::PlaceholderView;
pub use receive::ReceiveView;
pub use send::SendView;
pub use settings::SettingsView;
pub use unlock::UnlockView;
pub use wrap::WrapView;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

use crate::app::App;
use crate::brand;
use crate::input::Input;
use crate::jobs::{spinner_frame, ChromeFocus};
use alloy::primitives::U256;
use vaughan_core::chains::Balance;
use vaughan_core::core::{format_display_amount, parse_native_amount, OperatingMode};

/// True when `key` matches a footer chip (`v`/`d`/`g`, `x`, …).
///
/// Tab and Esc are intentionally excluded — Tab cycles field focus; Esc is
/// handled per view (deselect) then globally (Back).
pub(crate) fn is_footer_shortcut(key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if key.modifiers.contains(KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::SHIFT) {
        return false;
    }
    match key.code {
        KeyCode::Char('x') | KeyCode::Char('X') => !ctrl,
        KeyCode::Char(c) if c.is_ascii_alphabetic() => {
            matches!(
                c.to_ascii_lowercase(),
                'a' | 'b' | 'c' | 'd' | 'e' | 'f' | 'g' | 'h' | 'i' | 'j' | 'k' | 'l' | 'm'
                    | 'n' | 'o' | 'p' | 'r' | 's' | 't' | 'v' | 'w'
            )
        }
        _ => false,
    }
}

/// Which swap form field receives a picker (↑/↓) selection.
pub(crate) enum TokenFieldRole<'a> {
    InputIn { native_in: &'a mut bool },
    InputOut,
}

/// Apply a picked asset to Token in / Token out.
pub(crate) fn apply_picker_balance(
    balance: &Balance,
    role: TokenFieldRole<'_>,
    input: &mut Input,
    status: &mut String,
) -> bool {
    match role {
        TokenFieldRole::InputIn { native_in } => {
            if let Some(addr) = balance.token.contract_address.clone() {
                *native_in = false;
                input.set_value(addr);
            } else {
                *native_in = true;
                input.set_value("");
            }
            status.clear();
            true
        }
        TokenFieldRole::InputOut => {
            let Some(addr) = balance.token.contract_address.clone() else {
                *status = "Token out must be ERC-20".into();
                return true;
            };
            input.set_value(addr);
            status.clear();
            true
        }
    }
}

/// Sentinel: no ↑/↓ pick yet — first press selects index 0 (does not skip native).
pub(crate) const TOKEN_PICK_UNINIT: usize = usize::MAX;

/// Parse a token contract field; empty input gets a clear message (not alloy length errors).
pub(crate) fn parse_token_address(
    field: &str,
    label: &str,
) -> Result<alloy::primitives::Address, String> {
    use std::str::FromStr;
    let t = field.trim();
    if t.is_empty() {
        return Err(format!("{label}: pick with ↑↓ or paste a contract address"));
    }
    alloy::primitives::Address::from_str(t).map_err(|e| format!("{label}: {e}"))
}

/// Parse swap amounts: human decimals (`0.1`, `1`) or raw wei (`…wei` suffix or ≥15 digits).
pub(crate) fn parse_swap_amount(raw: &str, label: &str, decimals: u8) -> Result<U256, String> {
    use std::str::FromStr;
    let t = raw.trim();
    if t.is_empty() {
        return Err(format!("{label}: enter e.g. 0.1 or paste wei"));
    }
    let (strip, force_wei) = if let Some(s) = t.strip_suffix("wei") {
        (s.trim(), true)
    } else {
        (t, false)
    };
    if force_wei || (strip.chars().all(|c| c.is_ascii_digit()) && strip.len() >= 15) {
        let wei = U256::from_str(strip).map_err(|_| format!("{label}: invalid wei integer"))?;
        if wei.is_zero() {
            return Err(format!("{label}: must be > 0"));
        }
        return Ok(wei);
    }
    let wei_str = parse_native_amount(strip, decimals)
        .map_err(|e| format!("{label}: {}", e.user_message()))?;
    let wei = U256::from_str(&wei_str).map_err(|_| format!("{label}: parse failed"))?;
    if wei.is_zero() {
        return Err(format!("{label}: must be > 0"));
    }
    Ok(wei)
}

/// Min-out field: allows `0` / empty; human decimals (`1`, `0.01`) or raw wei (`1wei`, ≥15 digits).
pub(crate) fn parse_min_out_amount(raw: &str, label: &str, decimals: u8) -> Result<U256, String> {
    use std::str::FromStr;
    let t = raw.trim();
    if t.is_empty() || t == "0" {
        return Ok(U256::ZERO);
    }
    let (strip, force_wei) = if let Some(s) = t.strip_suffix("wei") {
        (s.trim(), true)
    } else {
        (t, false)
    };
    if force_wei || (strip.chars().all(|c| c.is_ascii_digit()) && strip.len() >= 15) {
        return U256::from_str(strip).map_err(|_| format!("{label}: invalid wei integer"));
    }
    let wei_str = parse_native_amount(strip, decimals)
        .map_err(|e| format!("{label}: {}", e.user_message()))?;
    U256::from_str(&wei_str).map_err(|_| format!("{label}: parse failed"))
}

fn picker_candidates(assets: &[Balance], out: bool) -> Vec<&Balance> {
    if out {
        assets
            .iter()
            .filter(|b| b.token.contract_address.is_some())
            .collect()
    } else {
        assets.iter().collect()
    }
}

/// ↑/↓ cycle through wallet assets into a focused Token in / Token out field.
pub(crate) fn cycle_token_picker(
    assets: &[Balance],
    out: bool,
    pick: &mut usize,
    forward: bool,
    native_in: &mut bool,
    input: &mut Input,
    status: &mut String,
) -> bool {
    let list = picker_candidates(assets, out);
    if list.is_empty() {
        *status = "No tokens loaded — open Assets or wait…".into();
        return true;
    }
    if *pick != TOKEN_PICK_UNINIT && *pick >= list.len() {
        *pick = TOKEN_PICK_UNINIT;
    }
    if *pick == TOKEN_PICK_UNINIT {
        *pick = 0;
    } else {
        *pick = if forward {
            (*pick + 1) % list.len()
        } else {
            (*pick + list.len() - 1) % list.len()
        };
    }
    let role = if out {
        TokenFieldRole::InputOut
    } else {
        TokenFieldRole::InputIn { native_in }
    };
    apply_picker_balance(list[*pick], role, input, status);
    *status = format!("{} · ↑↓ pick token", list[*pick].token.symbol);
    true
}

/// Resolve ticker symbol from the chrome asset list (case-insensitive address).
pub(crate) fn token_symbol_for_address<'a>(assets: &'a [Balance], addr: &str) -> Option<&'a str> {
    let want = addr.trim();
    if want.is_empty() {
        return None;
    }
    assets.iter().find_map(|b| {
        b.token
            .contract_address
            .as_ref()
            .filter(|a| a.eq_ignore_ascii_case(want))
            .map(|_| b.token.symbol.as_str())
    })
}

/// Known token tickers for common PulseChain addresses (when not in F2 list).
pub(crate) fn token_symbol_hint(addr: &str, chain_id: u64) -> Option<&'static str> {
    use vaughan_core::core::wiz4rd::WZRD_SMOKE_943;
    if addr.eq_ignore_ascii_case(WZRD_SMOKE_943) {
        return Some("WZRD");
    }
    let wpls = match chain_id {
        369 => "0xA1077a294dDE1B09bB078844df40758a5D0f9a27",
        943 => "0x70499adEBB11Efd915E3b69E700c331778628707",
        _ => "",
    };
    if !wpls.is_empty() && addr.eq_ignore_ascii_case(wpls) {
        return Some("WPLS");
    }
    None
}

/// Max fractional digits in the F2 chrome box (status strip).
const F2_FRAC_DIGITS: usize = 4;

/// Compact `amount symbol` for F2 (trim zeros, cap frac digits).
fn format_f2_balance(b: &Balance) -> String {
    format!(
        "{} {}",
        format_display_amount(&b.raw, b.token.decimals, F2_FRAC_DIGITS),
        b.token.symbol
    )
}

/// Render the full screen: wordmark, address, need-to-know status, body, footer.
///
/// While the vault is locked (welcome / unlock), chrome is omitted so the splash
/// can show only the big wordmark + password — no empty status boxes.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let unlocked = app.try_wallet().is_some_and(|w| w.is_unlocked());

    if !unlocked {
        let show_slogan = !app.suppress_unlock_slogan();
        let slogan_h = u16::from(show_slogan);
        let [body, slogan] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(slogan_h)]).areas(area);
        app.render_body(frame, body);
        if show_slogan {
            frame.render_widget(
                Paragraph::new(brand::typing_slogan(app.tick())).alignment(Alignment::Center),
                slogan,
            );
        }
        if app.quit_confirm().is_some() {
            render_quit_confirm(frame, area, app.quit_confirm() == Some(true));
        }
        return;
    }

    // Status boxes + blank gap + three rows of footer key chips when unlocked.
    let status_h = 3u16;
    let footer_h = 9u16;

    let [logo, _gap_above, addr_row, flash_row, status_bar, _gap_below_status, body, footer] =
        Layout::vertical([
            Constraint::Length(1), // VAUGHAN banner
            Constraint::Length(1), // blank above address
            Constraint::Length(1), // colour-coded address
            Constraint::Length(1), // copy toast / blank
            Constraint::Length(status_h),
            Constraint::Length(1), // blank under F1–F3
            Constraint::Min(0),
            Constraint::Length(footer_h),
        ])
        .areas(area);

    frame.render_widget(Paragraph::new(brand::logo_banner(logo.width)), logo);
    render_address_under_augha(frame, addr_row, app);
    render_chrome_flash(frame, flash_row, app);
    render_status_strip(frame, status_bar, app, unlocked);
    render_action_footer(frame, footer, app);
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

/// Copy / toast line under the address (visible on home and every unlocked screen).
fn render_chrome_flash(frame: &mut Frame, area: Rect, app: &App) {
    let Some(msg) = app.chrome().flash.as_deref() else {
        return;
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            msg.to_string(),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Center),
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

    let network_value = {
        use crate::mcp::McpListenerState;
        let mcp_tag = match chrome.mcp_listener {
            McpListenerState::Active if chrome.mcp_pending > 0 => {
                format!(" · MCP on ({})", chrome.mcp_pending)
            }
            McpListenerState::Active => " · MCP on".to_string(),
            McpListenerState::Starting => " · MCP …".to_string(),
            McpListenerState::Unavailable => " · MCP off".to_string(),
            McpListenerState::Off => String::new(),
        };
        // Sentient supersedes the adviser autonomy tier in the strip: the
        // agent signs on its own under policy, so `· Op` would mislead.
        let mode_tag = app
            .try_wallet()
            .map(|w| match w.operating_mode() {
                OperatingMode::SentientTrader => " · Sentient".to_string(),
                OperatingMode::HumanOnly => " · Human".to_string(),
                OperatingMode::AiAssisted => w
                    .agent_autonomy_tier()
                    .chrome_label()
                    .map(|l| format!(" · {l}"))
                    .unwrap_or_default(),
            })
            .unwrap_or_default();
        format!("{net_name}{testnet}{mcp_tag}{mode_tag}")
    };

    let asset_idx = if chrome.focus == ChromeFocus::Asset {
        chrome.pending_asset_idx.unwrap_or(chrome.asset_idx)
    } else {
        chrome.asset_idx
    };

    let token_value = if chrome.assets_loading && chrome.assets.is_empty() {
        format!("{} …", spinner_frame(app.tick()))
    } else if let Some(b) = chrome.assets.get(asset_idx) {
        format_f2_balance(b)
    } else if chrome.loading && chrome.balance.is_none() {
        format!("{} …", spinner_frame(app.tick()))
    } else {
        match &chrome.balance {
            Some(b) => format_f2_balance(b),
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

/// One faded square panel: bright-blue F1/F2/F3 title + centred value (accent ink).
/// When `focused`, title + value use reverse video so focus is obvious.
fn render_stat_box(frame: &mut Frame, area: Rect, title: &str, value: &str, focused: bool) {
    let mut title_style = Style::default()
        .fg(brand::action_key_color())
        .add_modifier(Modifier::BOLD);
    if focused {
        title_style = title_style.add_modifier(Modifier::REVERSED);
    }
    let title_line = Line::from(Span::styled(title.to_string(), title_style));
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
    // 21 chips → three rows of 7.
    let keys: &[(&str, &str)] = &[
        ("v", "Recv"),
        ("a", "Assets"),
        ("b", "Batch"),
        ("w", "Web"),
        ("c", "Browse"),
        ("d", "Dex"),
        ("g", "Ag"),
        ("h", "Home"),
        ("n", "Net"),
        ("i", "Settings"),
        ("k", "Keys"),
        ("e", "Wrap"),
        ("f", "Bridge"),
        ("j", "Appr"),
        ("m", "Hist"),
        ("p", "LP"),
        ("o", "NFT"),
        ("r", "Refresh"),
        ("l", "Lock"),
        ("tab", "Field"),
        ("esc", "Back"),
        ("x", "Quit"),
    ];

    let n = keys.len();
    debug_assert_eq!(n, 22);
    let row1_n = 7;
    let row2_n = 7;
    let [row1, row2, row3] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
    ])
    .spacing(0)
    .areas(area);

    render_chip_row(frame, row1, &keys[..row1_n]);
    render_chip_row(frame, row2, &keys[row1_n..row1_n + row2_n]);
    render_chip_row(frame, row3, &keys[row1_n + row2_n..]);
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
                .fg(brand::action_key_color())
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

/// Render a labelled text input with a bright-blue Fn key in the title (F4 / F5).
///
/// Matches chrome F1–F3 key colour so home send fields read as hotkeyed boxes.
pub(crate) fn render_fkey_labeled_input(
    frame: &mut Frame,
    area: Rect,
    fkey: &str,
    label: &str,
    input: &Input,
    focused: bool,
) {
    let mut key_style = Style::default()
        .fg(brand::action_key_color())
        .add_modifier(Modifier::BOLD);
    let mut label_style = Style::default().fg(brand::body_color());
    if focused {
        key_style = key_style.add_modifier(Modifier::REVERSED);
        label_style = Style::default()
            .fg(brand::accent_color())
            .add_modifier(Modifier::BOLD | Modifier::REVERSED);
    }
    let title = Line::from(vec![
        Span::styled(format!(" {fkey} "), key_style),
        Span::styled(format!("{label} "), label_style),
    ]);
    let inner = brand::render_faded_box(frame, area, Some(title));
    if !focused && input.value().is_empty() {
        return;
    }
    frame.render_widget(Paragraph::new(input.line()), inner);
}

/// True when the user typed or deleted in a token field — picker index must reset.
pub(crate) fn manual_edit_resets_token_pick(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
    )
}

/// Render a labelled text input inside a faded square box (yellow title when focused).
pub(crate) fn render_labeled_input(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    input: &Input,
    focused: bool,
) {
    render_labeled_input_aligned(frame, area, label, input, focused, Alignment::Left);
}

/// Like [`render_labeled_input`] with horizontal alignment for the field body.
pub(crate) fn render_labeled_input_aligned(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    input: &Input,
    focused: bool,
    align: Alignment,
) {
    let title_text = format!(" {label} ");
    let title = if focused {
        brand::focus_title(&title_text)
    } else {
        brand::fade_line(&title_text)
    };
    let inner = brand::render_faded_box(frame, area, Some(title));
    if input.value().is_empty() {
        if focused {
            frame.render_widget(Paragraph::new(input.line()).alignment(align), inner);
        } else {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    input.placeholder(),
                    Style::default().fg(Color::DarkGray),
                )))
                .alignment(align),
                inner,
            );
        }
        return;
    }
    frame.render_widget(Paragraph::new(input.line()).alignment(align), inner);
}

/// Human-readable native symbol for the active EVM chain.
pub(crate) fn native_pls_label(chain_id: u64) -> &'static str {
    match chain_id {
        943 => "tPLS",
        369 => "PLS",
        _ => "PLS",
    }
}

/// A status/error line rendered at the bottom of a view's body.
pub(crate) fn status_paragraph(status: &str) -> Paragraph<'static> {
    let style = if status.is_empty() {
        Style::default()
    } else {
        Style::default().fg(Color::Red)
    };
    Paragraph::new(Span::styled(
        status.to_string(),
        style,
    ))
}

/// Split a view body into content + a status line.
pub(crate) fn body_areas(area: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_picker_balance, cycle_token_picker, fit_address, fit_raw, parse_min_out_amount,
        parse_swap_amount, parse_token_address, TokenFieldRole, TOKEN_PICK_UNINIT,
    };
    use crate::input::Input;
    use alloy::primitives::U256;
    use std::str::FromStr;
    use vaughan_core::chains::{Balance, TokenInfo};

    #[test]
    fn apply_picker_native_clears_token_in() {
        let native = Balance {
            token: TokenInfo {
                symbol: "PLS".into(),
                name: "PulseChain".into(),
                decimals: 18,
                contract_address: None,
            },
            raw: "1".into(),
            formatted: "1".into(),
            usd_value: None,
        };
        let mut native_in = false;
        let mut input = Input::new(false, "");
        input.set_value("0xdead");
        let mut status = String::new();
        assert!(apply_picker_balance(
            &native,
            TokenFieldRole::InputIn {
                native_in: &mut native_in,
            },
            &mut input,
            &mut status,
        ));
        assert!(native_in);
        assert!(input.value().is_empty());
    }

    #[test]
    fn apply_picker_erc20_sets_token_out() {
        let token = Balance {
            token: TokenInfo {
                symbol: "MEME".into(),
                name: "Meme".into(),
                decimals: 18,
                contract_address: Some("0x2222222222222222222222222222222222222222".into()),
            },
            raw: "1".into(),
            formatted: "1".into(),
            usd_value: None,
        };
        let mut input = Input::new(false, "");
        let mut status = String::new();
        assert!(apply_picker_balance(
            &token,
            TokenFieldRole::InputOut,
            &mut input,
            &mut status,
        ));
        assert!(input.value().contains("2222"));
    }

    #[test]
    fn parse_token_address_rejects_empty() {
        assert!(parse_token_address("", "Token out").is_err());
        assert!(parse_token_address("  ", "Token out")
            .unwrap_err()
            .contains("↑↓"));
    }

    #[test]
    fn parse_swap_amount_human_and_wei() {
        assert_eq!(
            parse_swap_amount("0.1", "amount", 18).unwrap(),
            U256::from_str("100000000000000000").unwrap()
        );
        assert_eq!(
            parse_swap_amount("1", "amount", 18).unwrap(),
            U256::from_str("1000000000000000000").unwrap()
        );
        let wei = U256::from_str("1000000000000000000").unwrap();
        assert_eq!(
            parse_swap_amount("1000000000000000000", "amount", 18).unwrap(),
            wei
        );
        assert_eq!(
            parse_swap_amount("1000000000000000000wei", "amount", 18).unwrap(),
            wei
        );
    }

    #[test]
    fn parse_min_out_human_and_wei() {
        assert_eq!(
            parse_min_out_amount("1", "min out", 18).unwrap(),
            U256::from(10u128.pow(18))
        );
        assert_eq!(
            parse_min_out_amount("0.01", "min out", 18).unwrap(),
            U256::from(10u128.pow(16))
        );
        assert_eq!(
            parse_min_out_amount("1wei", "min out", 18).unwrap(),
            U256::from(1)
        );
        assert_eq!(
            parse_min_out_amount("0", "min out", 18).unwrap(),
            U256::ZERO
        );
    }

    #[test]
    fn cycle_token_picker_first_press_selects_index_zero() {
        let assets = vec![
            Balance {
                token: TokenInfo {
                    symbol: "PLS".into(),
                    name: "PulseChain".into(),
                    decimals: 18,
                    contract_address: None,
                },
                raw: "1".into(),
                formatted: "1".into(),
                usd_value: None,
            },
            Balance {
                token: TokenInfo {
                    symbol: "PLSX".into(),
                    name: "PLSX".into(),
                    decimals: 18,
                    contract_address: Some("0x95B303987A60C71504D99Aa1b13B4DA07b0790ab".into()),
                },
                raw: "1".into(),
                formatted: "1".into(),
                usd_value: None,
            },
        ];
        let mut pick = TOKEN_PICK_UNINIT;
        let mut native_in = false;
        let mut input = Input::new(false, "");
        let mut status = String::new();
        cycle_token_picker(
            &assets,
            false,
            &mut pick,
            true,
            &mut native_in,
            &mut input,
            &mut status,
        );
        assert_eq!(pick, 0);
        assert!(native_in);
        assert!(input.value().is_empty());
        cycle_token_picker(
            &assets,
            false,
            &mut pick,
            true,
            &mut native_in,
            &mut input,
            &mut status,
        );
        assert_eq!(pick, 1);
        assert!(!native_in);
        assert!(input.value().contains("95B303"));
    }

    #[test]
    fn cycle_token_picker_advances_out_list() {
        let assets = vec![
            Balance {
                token: TokenInfo {
                    symbol: "PLS".into(),
                    name: "PulseChain".into(),
                    decimals: 18,
                    contract_address: None,
                },
                raw: "1".into(),
                formatted: "1".into(),
                usd_value: None,
            },
            Balance {
                token: TokenInfo {
                    symbol: "PLSX".into(),
                    name: "PLSX".into(),
                    decimals: 18,
                    contract_address: Some("0x95B303987A60C71504D99Aa1b13B4DA07b0790ab".into()),
                },
                raw: "1".into(),
                formatted: "1".into(),
                usd_value: None,
            },
        ];
        let mut pick = TOKEN_PICK_UNINIT;
        let mut native_in = true;
        let mut input = Input::new(false, "");
        let mut status = String::new();
        cycle_token_picker(
            &assets,
            true,
            &mut pick,
            true,
            &mut native_in,
            &mut input,
            &mut status,
        );
        assert_eq!(pick, 0);
        assert!(input.value().contains("95B303"));
        cycle_token_picker(
            &assets,
            true,
            &mut pick,
            true,
            &mut native_in,
            &mut input,
            &mut status,
        );
        assert_eq!(pick, 0); // only one ERC-20 candidate for out
    }

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
