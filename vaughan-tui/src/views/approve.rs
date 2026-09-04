//! Approve: the full-screen prompt shown when a dApp requests a sign/send.
//!
//! This view is render-only for the request itself. It displays exactly what
//! will be signed (method, origin, recipient, value, chain, data) and tells
//! the user which keys approve (`y`/Enter) or deny (`n`/Esc). The decision and
//! the actual signing are handled by [`crate::app::App`], which owns the
//! pending request's reply channel and the wallet state.
//!
//! Transaction prompts (send / sign-only) additionally carry a fee editor:
//! speed presets `1`–`5` (Slow … Custom, same model as the Send view) adjust
//! the fee shown in the prompt, and the adjusted fee is applied to the
//! transaction's gas fields when the user approves.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::chains::{Fee, FeeSpeed};
use vaughan_core::core::{format_base_units, WalletState};
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::brand;
use crate::input::{Input, InputAction};

/// Drop control characters (C0/C1/DEL) and Unicode *format* characters (Cf)
/// from display-bound text. `char::is_control` covers only Cc; Cf chars —
/// bidi overrides/isolates, zero-width spaces, tag chars — survive it and can
/// visually reorder or hide parts of an approval prompt (origin/agent strings
/// are attacker-influenced), so they are stripped here too.
fn sanitize_display(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control() && !is_unicode_format(*c))
        .collect()
}

/// Unicode general-category Cf (format) code points relevant to prompt
/// deception: bidi controls, zero-widths, invisible operators, tag chars.
/// Ranges follow the Unicode Cf table; no new dependency for one predicate.
fn is_unicode_format(c: char) -> bool {
    matches!(c,
        '\u{00AD}'
        | '\u{0600}'..='\u{0605}'
        | '\u{061C}'
        | '\u{06DD}'
        | '\u{070F}'
        | '\u{0890}'..='\u{0891}'
        | '\u{08E2}'
        | '\u{180E}'
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}'
        | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}'
        | '\u{110BD}'
        | '\u{110CD}'
        | '\u{13430}'..='\u{1343F}'
        | '\u{1BCA0}'..='\u{1BCA3}'
        | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0001}'
        | '\u{E0020}'..='\u{E007F}')
}

/// How the fee editor answered a key: `Blocked` means the key looked like an
/// approve/deny decision but the editor vetoed it (invalid custom gwei, or
/// Esc merely unfocusing the input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeKeyOutcome {
    NotHandled,
    Consumed,
    Blocked,
}

/// Fee preset/custom state for transaction approvals (mirrors the Send view).
struct FeeEdit {
    /// Unscaled prompt-time estimate (explicit tx gas fields or RPC estimate).
    base: Fee,
    speed: FeeSpeed,
    /// Custom max fee in gwei (only when [`FeeSpeed::Custom`] is selected).
    custom_gas: Input,
    custom_focus: bool,
    /// Index of the `Fee:` line inside `details`, rewritten on speed change.
    fee_line: usize,
    error: Option<String>,
}

impl FeeEdit {
    fn new(base: Fee, fee_line: usize) -> Self {
        Self {
            base,
            speed: FeeSpeed::Normal,
            custom_gas: Input::new(false, "gwei"),
            custom_focus: false,
            fee_line,
            error: None,
        }
    }

    fn adjusted(&self) -> Fee {
        match self.speed {
            FeeSpeed::Custom => self
                .base
                .with_custom_max_fee_gwei(self.custom_gas.value())
                .unwrap_or_else(|_| self.base.clone()),
            speed => self.base.with_speed(speed),
        }
    }

    fn select_speed(&mut self, speed: FeeSpeed) {
        self.speed = speed;
        if speed == FeeSpeed::Custom {
            if self.custom_gas.value().is_empty() {
                if let Some(gwei) = max_fee_gwei_display(&self.base) {
                    self.custom_gas.set_value(gwei);
                }
            }
            self.custom_focus = true;
        }
    }
}

/// Base max fee rendered in gwei, for prefilling the custom input.
fn max_fee_gwei_display(fee: &Fee) -> Option<String> {
    match &fee.details {
        vaughan_core::chains::FeeDetails::Evm {
            max_fee_per_gas: Some(wei),
            ..
        } => {
            let s = format_base_units(wei, 9);
            if s.is_empty() || s == "0" {
                None
            } else {
                Some(s)
            }
        }
        _ => None,
    }
}

pub struct ApproveView {
    title: String,
    origin: Option<String>,
    details: Vec<String>,
    /// Decoded human verification rows (LP Brew, etc.) rendered as a bordered table.
    verify_table: Vec<(String, String)>,
    fee: Option<FeeEdit>,
}

impl ApproveView {
    pub fn new(
        title: String,
        origin: Option<String>,
        details: Vec<String>,
        verify_table: Vec<(String, String)>,
    ) -> Self {
        // All fields can carry remote-controlled text (page origin, site
        // key, sign message, MCP agent explanation). Strip control chars so a
        // malicious dApp cannot inject terminal escape sequences (OSC-8 links,
        // screen repaints) into the prompt the user approves from.
        Self {
            title: sanitize_display(&title),
            origin: origin.map(|o| sanitize_display(&o)),
            details: details.iter().map(|d| sanitize_display(d)).collect(),
            verify_table: verify_table
                .into_iter()
                .map(|(label, value)| (sanitize_display(&label), sanitize_display(&value)))
                .collect(),
            fee: None,
        }
    }

    /// Transaction prompt variant with a fee editor seeded from the
    /// prompt-time fee. The `Fee:` line produced by `describe_tx` is located
    /// once and rewritten as the user changes speed.
    pub fn with_fee(
        title: String,
        origin: Option<String>,
        details: Vec<String>,
        verify_table: Vec<(String, String)>,
        base_fee: Fee,
    ) -> Self {
        let mut view = Self::new(title, origin, details, verify_table);
        if let Some(line) = view.details.iter().position(|l| l.starts_with("Fee:")) {
            view.fee = Some(FeeEdit::new(base_fee, line));
        }
        view
    }

    pub fn has_fee_editor(&self) -> bool {
        self.fee.is_some()
    }

    /// The fee as adjusted by the user's speed/custom selection.
    pub fn adjusted_fee(&self) -> Option<Fee> {
        self.fee.as_ref().map(FeeEdit::adjusted)
    }

    /// First crack at keys while a fee editor is present. `App` falls through
    /// to its approve/deny handling only on [`FeeKeyOutcome::NotHandled`].
    pub fn handle_fee_key(&mut self, key: KeyEvent) -> FeeKeyOutcome {
        let Some(edit) = self.fee.as_mut() else {
            return FeeKeyOutcome::NotHandled;
        };
        match key.code {
            KeyCode::Up => {
                edit.select_speed(edit.speed.prev());
                self.refresh_fee_line();
                FeeKeyOutcome::Consumed
            }
            KeyCode::Down => {
                edit.select_speed(edit.speed.next());
                self.refresh_fee_line();
                FeeKeyOutcome::Consumed
            }
            KeyCode::Char(c) if FeeSpeed::from_digit(c).is_some() && !edit.custom_focus => {
                edit.select_speed(FeeSpeed::from_digit(c).unwrap_or_default());
                self.refresh_fee_line();
                FeeKeyOutcome::Consumed
            }
            KeyCode::Tab if edit.speed == FeeSpeed::Custom => {
                edit.custom_focus = !edit.custom_focus;
                FeeKeyOutcome::Consumed
            }
            KeyCode::Esc if edit.custom_focus => {
                edit.custom_focus = false;
                FeeKeyOutcome::Blocked
            }
            KeyCode::Enter if edit.speed == FeeSpeed::Custom => {
                match edit.base.with_custom_max_fee_gwei(edit.custom_gas.value()) {
                    Ok(_) => {
                        edit.error = None;
                        edit.custom_focus = false;
                        self.refresh_fee_line();
                        FeeKeyOutcome::NotHandled
                    }
                    Err(e) => {
                        edit.error = Some(e);
                        edit.custom_focus = true;
                        FeeKeyOutcome::Blocked
                    }
                }
            }
            _ if edit.custom_focus => match key.code {
                // A gwei field only needs digits and '.', so decision keys
                // must never be swallowed as text: deny always falls through;
                // approve falls through only when the custom value validates.
                KeyCode::Char('n') | KeyCode::Char('N') => FeeKeyOutcome::NotHandled,
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    match edit.base.with_custom_max_fee_gwei(edit.custom_gas.value()) {
                        Ok(_) => {
                            edit.error = None;
                            self.refresh_fee_line();
                            FeeKeyOutcome::NotHandled
                        }
                        Err(e) => {
                            edit.error = Some(e);
                            FeeKeyOutcome::Blocked
                        }
                    }
                }
                _ => match edit.custom_gas.handle_key(key) {
                    InputAction::Consumed => {
                        edit.error = None;
                        self.refresh_fee_line();
                        FeeKeyOutcome::Consumed
                    }
                    // Enter is matched above; treat Submitted the same as any
                    // other consumed edit key.
                    InputAction::Submitted => FeeKeyOutcome::Consumed,
                    // While the input is focused, stray keys must not fall
                    // through to approve/deny.
                    InputAction::Ignored => FeeKeyOutcome::Blocked,
                },
            },
            _ => FeeKeyOutcome::NotHandled,
        }
    }

    fn refresh_fee_line(&mut self) {
        if let Some(edit) = self.fee.as_ref() {
            let line = format!("Fee:     {}", edit.adjusted().total);
            let idx = edit.fee_line;
            if let Some(slot) = self.details.get_mut(idx) {
                *slot = line;
            }
        }
    }

    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        _wallet: &WalletState,
        signing: bool,
        tick: u64,
    ) {
        let origin = self.origin.as_deref().unwrap_or("(no origin)");
        let mut text = vec![
            Line::from(Span::styled(
                self.title.clone(),
                Style::default().fg(Color::Yellow),
            )),
            Line::from(format!("Origin:  {origin}")),
            Line::from(""),
        ];
        if !self.verify_table.is_empty() {
            text.push(Line::from(Span::styled(
                "Summary",
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD),
            )));
            for line in verify_table_compact_lines(&self.verify_table, area.width.saturating_sub(4))
            {
                text.push(line);
            }
            text.push(Line::from(""));
        }
        for detail in &self.details {
            text.push(Line::from(detail.clone()));
        }
        if let Some(edit) = &self.fee {
            text.push(Line::from(""));
            text.push(Line::from(format!(
                "Fee speed: {}   (1 Slow · 2 Normal · 3 Fast · 4 Ape · 5 Custom, ↑/↓ cycle)",
                edit.speed.label()
            )));
            if edit.speed == FeeSpeed::Custom {
                text.push(edit.custom_gas.line());
            }
            if let Some(error) = &edit.error {
                text.push(Line::from(Span::styled(
                    error.clone(),
                    Style::default().fg(Color::Red),
                )));
            }
        }
        text.push(Line::from(""));
        if signing {
            text.push(Line::from(Span::styled(
                format!(
                    "{} Signing and broadcasting — please wait…",
                    crate::jobs::spinner_frame(tick)
                ),
                Style::default().fg(Color::Yellow),
            )));
            text.push(Line::from(""));
        } else {
            text.push(Line::from(
                "y / Enter — approve (after brief pause)     n / Esc — deny",
            ));
        }
        text.push(Line::from(
            "Stale prompts auto-deny after 60s (dApp timeout safety).",
        ));

        let inner =
            brand::render_faded_box(frame, area, Some(brand::fade_line(" Approve request ")));
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
    }

    pub fn allows_footer_shortcuts(&self) -> bool {
        false
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

/// Compact styled lines for the approve screen (fits narrow terminals).
pub fn verify_table_compact_lines(rows: &[(String, String)], width: u16) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return Vec::new();
    }
    let inner = width.max(40) as usize;
    const LABEL_W: usize = 12;
    let mut out = Vec::with_capacity(rows.len() + 1);

    let job = rows
        .iter()
        .find(|(label, _)| label == "Brew job")
        .map(|(_, value)| value.as_str());
    let step = rows
        .iter()
        .find(|(label, _)| label == "Step")
        .map(|(_, value)| value.as_str());
    if job.is_some() || step.is_some() {
        let mut meta = String::new();
        if let Some(s) = step {
            meta.push_str(s);
        }
        if let Some(j) = job {
            if !meta.is_empty() {
                meta.push_str(" · ");
            }
            meta.push_str("job ");
            meta.push_str(j);
        }
        out.push(Line::from(Span::styled(
            truncate_display(&meta, inner),
            Style::default().fg(Color::DarkGray),
        )));
    }

    for (label, value) in rows {
        if matches!(label.as_str(), "Brew job" | "Step" | "Recipient") {
            continue;
        }
        let value_style = if label.starts_with("Deposit") {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if label == "Pool" {
            Style::default()
                .fg(brand::accent_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(brand::body_color())
        };
        out.push(compact_verify_row(
            label,
            value,
            inner,
            LABEL_W,
            Style::default().fg(brand::accent_color()),
            value_style,
        ));
    }
    out
}

fn compact_verify_row(
    label: &str,
    value: &str,
    inner_width: usize,
    label_w: usize,
    label_style: Style,
    value_style: Style,
) -> Line<'static> {
    let value_w = inner_width.saturating_sub(label_w + 2).max(12);
    Line::from(vec![
        Span::styled(
            format!(" {:label_w$}", truncate_display(label, label_w)),
            label_style,
        ),
        Span::raw(" "),
        Span::styled(truncate_display(value, value_w), value_style),
    ])
}

/// Render verification rows as a Unicode box table (success flash under address).
pub fn verify_table_lines(rows: &[(String, String)], width: u16) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return Vec::new();
    }
    let inner = width.max(40) as usize;
    let label_w = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(8)
        .clamp(8, 24);
    let value_w = inner.saturating_sub(label_w + 3).max(16);
    let horiz = |w: usize| "─".repeat(w);
    let mut out = Vec::with_capacity(rows.len() * 2 + 2);
    out.push(Line::from(format!(
        "┌─{}─┬─{}─┐",
        horiz(label_w),
        horiz(value_w)
    )));
    for (idx, (label, value)) in rows.iter().enumerate() {
        out.push(Line::from(format!(
            "│ {:label_w$} │ {:value_w$} │",
            truncate_display(label, label_w),
            truncate_display(value, value_w),
            label_w = label_w,
            value_w = value_w
        )));
        if idx + 1 < rows.len() {
            out.push(Line::from(format!(
                "├─{}─┼─{}─┤",
                horiz(label_w),
                horiz(value_w)
            )));
        }
    }
    out.push(Line::from(format!(
        "└─{}─┴─{}─┘",
        horiz(label_w),
        horiz(value_w)
    )));
    out
}

fn truncate_display(raw: &str, max_chars: usize) -> String {
    let chars: Vec<char> = raw.chars().collect();
    if chars.len() <= max_chars {
        return raw.to_string();
    }
    if max_chars <= 1 {
        return "…".into();
    }
    format!("{}…", chars[..max_chars - 1].iter().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use vaughan_core::chains::FeeDetails;

    fn evm_fee(max_wei: &str, tip_wei: Option<&str>) -> Fee {
        Fee {
            total: "0.021 tPLS".into(),
            currency: "tPLS".into(),
            details: FeeDetails::Evm {
                gas_limit: 50_000,
                max_fee_per_gas: Some(max_wei.into()),
                max_priority_fee_per_gas: tip_wei.map(Into::into),
            },
        }
    }

    fn tx_view() -> ApproveView {
        ApproveView::with_fee(
            "Sign & broadcast transaction".into(),
            Some("https://dapp.example".into()),
            vec![
                "To:      0xabc".into(),
                "Value:   1 tPLS".into(),
                "Network: PulseChain Testnet v4 (testnet)".into(),
                "Fee:     0.021 tPLS".into(),
            ],
            vec![],
            evm_fee("420000000", Some("100000000")),
        )
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn verify_table_compact_fits_fewer_lines_than_box() {
        let rows = vec![
            ("Brew job".into(), "lp_abc".into()),
            ("Step".into(), "Add liquidity (mint)".into()),
            ("Pool".into(), "BOB / JANE · Wiz4rd · 2% fee".into()),
            ("Range".into(), "Full (-887200 → 887200)".into()),
            ("Deposit BOB".into(), "100".into()),
            ("Deposit JANE".into(), "0.04".into()),
            ("Recipient".into(), "0x9274…".into()),
        ];
        let compact = verify_table_compact_lines(&rows, 80);
        let boxed = verify_table_lines(&rows, 80);
        assert!(compact.len() < boxed.len());
        assert!(compact.iter().any(|l| l.to_string().contains("BOB / JANE")));
        assert!(!compact.iter().any(|l| l.to_string().starts_with('┌')));
    }

    #[test]
    fn verify_table_renders_box_rows() {
        let lines = verify_table_lines(
            &[
                ("Pool".into(), "T1 / T2 · wiz4rd · 2% fee".into()),
                ("Range".into(), "Full (-887200 → 887200)".into()),
            ],
            80,
        );
        assert!(lines[0].to_string().starts_with('┌'));
        assert!(lines.iter().any(|l| l.to_string().contains("T1 / T2")));
        assert!(lines.last().unwrap().to_string().starts_with('└'));
    }

    #[test]
    fn sanitize_display_strips_escape_sequences() {
        // OSC-8 hyperlink + newline injection must not survive.
        let evil = "https://a.example\u{1b}]8;;https://evil.example\u{7}\nfake-line";
        let clean = sanitize_display(evil);
        assert!(!clean.chars().any(|c| c.is_control()));
        assert!(clean.contains("https://a.example"));
        assert!(clean.contains("fake-line"));
    }

    #[test]
    fn sanitize_display_strips_bidi_and_zero_width() {
        // RLO visual-reorder + ZWSP + bidi isolate + BOM must not survive;
        // ordinary text and non-ASCII letters are kept.
        let evil =
            "https://trusted.example\u{202E}moc.elpmaxe\u{202C}\u{200B}\u{2066}x\u{2069}\u{FEFF}";
        let clean = sanitize_display(evil);
        assert!(!clean.chars().any(is_unicode_format));
        assert!(clean.starts_with("https://trusted.example"));
        let cyrillic = sanitize_display("пaypаl.example");
        assert_eq!(cyrillic, "пaypаl.example");
    }

    #[test]
    fn view_construction_sanitizes_all_fields() {
        let view = ApproveView::new(
            "T\u{1b}[31mitle".into(),
            Some("https://x.example\u{1b}]8;;x\u{7}".into()),
            vec!["Message: hello\u{1b}[2J".into()],
            vec![],
        );
        assert!(!view.title.chars().any(|c| c.is_control()));
        assert!(!view.origin.unwrap().chars().any(|c| c.is_control()));
        assert!(!view.details[0].chars().any(|c| c.is_control()));
        assert!(view.details[0].contains("Message: hello"));
    }

    #[test]
    fn fee_editor_attaches_to_fee_line() {
        let view = tx_view();
        assert!(view.has_fee_editor());
        assert_eq!(view.fee.as_ref().map(|f| f.fee_line), Some(3));
    }

    #[test]
    fn non_transaction_prompt_has_no_fee_editor() {
        let mut view = ApproveView::new(
            "Sign message".into(),
            None,
            vec!["Message: hi".into()],
            vec![],
        );
        assert!(!view.has_fee_editor());
        assert_eq!(
            view.handle_fee_key(key(KeyCode::Char('3'))),
            FeeKeyOutcome::NotHandled
        );
    }

    #[test]
    fn digit_selects_speed_and_updates_fee_line() {
        let mut view = tx_view();
        assert_eq!(
            view.handle_fee_key(key(KeyCode::Char('3'))),
            FeeKeyOutcome::Consumed
        );
        // Fast = 125% of the 0.42 gwei base max fee.
        let fee = view.adjusted_fee().unwrap();
        match &fee.details {
            FeeDetails::Evm {
                max_fee_per_gas, ..
            } => assert_eq!(max_fee_per_gas.as_deref(), Some("525000000")),
            _ => panic!("expected EVM fee details"),
        }
        assert!(view.details[3].starts_with("Fee:     "));
    }

    #[test]
    fn custom_speed_focuses_input_and_validates_on_enter() {
        let mut view = tx_view();
        view.handle_fee_key(key(KeyCode::Char('5')));
        let edit = view.fee.as_ref().unwrap();
        assert!(edit.custom_focus);
        // Prefilled with the base max fee in gwei.
        assert!(!edit.custom_gas.value().is_empty());

        // Clear and enter garbage: Enter must block the approval.
        view.fee.as_mut().unwrap().custom_gas.set_value("abc");
        assert_eq!(
            view.handle_fee_key(key(KeyCode::Enter)),
            FeeKeyOutcome::Blocked
        );
        assert!(view.fee.as_ref().unwrap().error.is_some());

        // Valid value: Enter unblocks and unfocuses.
        view.fee.as_mut().unwrap().custom_gas.set_value("2.5");
        assert_eq!(
            view.handle_fee_key(key(KeyCode::Enter)),
            FeeKeyOutcome::NotHandled
        );
        let fee = view.adjusted_fee().unwrap();
        match &fee.details {
            FeeDetails::Evm {
                max_fee_per_gas, ..
            } => assert_eq!(max_fee_per_gas.as_deref(), Some("2500000000")),
            _ => panic!("expected EVM fee details"),
        }
    }

    #[test]
    fn esc_while_custom_focused_only_unfocuses() {
        let mut view = tx_view();
        view.handle_fee_key(key(KeyCode::Char('5')));
        assert_eq!(
            view.handle_fee_key(key(KeyCode::Esc)),
            FeeKeyOutcome::Blocked
        );
        assert!(!view.fee.as_ref().unwrap().custom_focus);
        // Second Esc falls through to App's deny handling.
        assert_eq!(
            view.handle_fee_key(key(KeyCode::Esc)),
            FeeKeyOutcome::NotHandled
        );
    }

    #[test]
    fn decision_keys_work_while_custom_input_focused() {
        let mut view = tx_view();
        view.handle_fee_key(key(KeyCode::Char('5')));
        // Valid prefill: 'y' falls through so App can approve.
        assert_eq!(
            view.handle_fee_key(key(KeyCode::Char('y'))),
            FeeKeyOutcome::NotHandled
        );
        // Invalid value: 'y' is blocked with an error, not swallowed as text.
        view.fee.as_mut().unwrap().custom_gas.set_value("abc");
        view.fee.as_mut().unwrap().custom_focus = true;
        assert_eq!(
            view.handle_fee_key(key(KeyCode::Char('y'))),
            FeeKeyOutcome::Blocked
        );
        assert!(view.fee.as_ref().unwrap().error.is_some());
        // Deny always falls through.
        assert_eq!(
            view.handle_fee_key(key(KeyCode::Char('n'))),
            FeeKeyOutcome::NotHandled
        );
    }
}
