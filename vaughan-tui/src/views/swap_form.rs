//! Dex-style swap form chrome shared by Dex, Aggregator, and Bridge views.
//!
//! Field boxes with inner grey labels, accent border when focused, and centered
//! read-only values where appropriate.

use alloy::primitives::U256;
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use vaughan_core::chains::Balance;
use vaughan_core::core::format_display_amount;

use crate::brand;
use crate::input::Input;
use crate::views::{native_pls_label, token_symbol_for_address, token_symbol_hint};

/// Longest swap field label — value column starts after this width.
pub const SWAP_LABEL_WIDTH: usize = 16;

/// Max fractional digits for display amounts (DEX-style).
pub const SWAP_DISPLAY_FRAC: usize = 5;

pub fn field_label_style() -> Style {
    Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::BOLD)
}

pub fn field_label_span(label: &str) -> Span<'static> {
    Span::styled(
        format!("{:<SWAP_LABEL_WIDTH$} ", label),
        field_label_style(),
    )
}

pub fn field_label_short_span(label: &str) -> Span<'static> {
    Span::styled(format!("{label} "), field_label_style())
}

/// Centered title row (e.g. ` Swap `, ` Ag `).
pub fn render_form_title(frame: &mut Frame, area: Rect, title: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            title,
            Style::default()
                .fg(brand::accent_color())
                .add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Center),
        area,
    );
}

/// Venue / protocol selector line under the title.
pub fn render_selector_line(
    frame: &mut Frame,
    area: Rect,
    primary: Line<'_>,
    hint: &str,
    focused: bool,
) {
    let primary_style = if focused {
        Style::default()
            .fg(brand::accent_color())
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(brand::body_color())
    };
    let mut spans = primary.spans;
    for span in &mut spans {
        span.style = primary_style;
    }
    spans.push(Span::styled(
        format!("  {hint}"),
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        area,
    );
}

/// Down arrow between In and Out legs.
pub fn render_leg_arrow(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "↓",
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Center),
        area,
    );
}

/// Human-readable swap amount (no raw wei).
pub fn fmt_swap_wei_amount(wei: &U256, decimals: u8) -> String {
    format_display_amount(&wei.to_string(), decimals, SWAP_DISPLAY_FRAC)
}

/// Token ticker for confirm screens — native label or address lookup.
pub fn token_display_symbol(
    native: bool,
    token_input: &Input,
    assets: &[Balance],
    chain_id: u64,
) -> String {
    if native {
        return native_pls_label(chain_id).to_string();
    }
    let raw = token_input.value().trim();
    if raw.is_empty() {
        return "???".to_string();
    }
    token_symbol_for_address(assets, raw)
        .or_else(|| token_symbol_hint(raw, chain_id))
        .unwrap_or("TOKEN")
        .to_string()
}

/// Copy-friendly confirm panel — plain text, no box borders.
pub fn render_plain_confirm(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    body: Vec<Line<'static>>,
    footer: &str,
) {
    let [title_row, body_row, footer_row] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .margin(1)
    .areas(area);

    render_form_title(frame, title_row, title);

    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: true }), body_row);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            footer,
            Style::default().fg(Color::DarkGray),
        ))),
        footer_row,
    );
}

/// Footer hint centred under the form fields.
pub fn render_form_footer(frame: &mut Frame, area: Rect, hint: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Center),
        area,
    );
}

/// Labelled text input inside a field box.
pub fn render_text_field(frame: &mut Frame, area: Rect, label: &str, input: &Input, focused: bool) {
    let inner = brand::render_field_box(frame, area, focused);
    let mut spans = vec![field_label_span(label)];
    if input.value().is_empty() {
        if focused {
            spans.extend(input.line().spans);
        } else {
            spans.push(Span::styled(
                input.placeholder(),
                Style::default().fg(Color::DarkGray),
            ));
        }
    } else if focused {
        spans.extend(input.line().spans);
    } else {
        spans.push(Span::raw(input.value()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

/// Read-only centred value inside a field box (native PLS, chain preset, etc.).
pub fn render_centered_value_field(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    value: Line<'_>,
    focused: bool,
) {
    let inner = brand::render_field_box(frame, area, focused);
    frame.render_widget(Paragraph::new(value).alignment(Alignment::Center), inner);
    frame.render_widget(
        Paragraph::new(Line::from(field_label_span(label))),
        Rect {
            x: inner.x,
            y: inner.y,
            width: (SWAP_LABEL_WIDTH + 1) as u16,
            height: 1,
        },
    );
}

/// Label on the left; amount + suffix centred across the full row width.
pub fn render_centered_amount_row(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    value: Line<'_>,
    focused: bool,
    bordered: bool,
) {
    let inner = if bordered {
        brand::render_field_box(frame, area, focused)
    } else {
        Rect {
            x: area.x.saturating_add(1),
            y: area.y,
            width: area.width.saturating_sub(2),
            height: area.height.max(1),
        }
    };
    frame.render_widget(Paragraph::new(value).alignment(Alignment::Center), inner);
    frame.render_widget(
        Paragraph::new(Line::from(field_label_span(label))),
        Rect {
            x: inner.x,
            y: inner.y,
            width: (SWAP_LABEL_WIDTH + 1) as u16,
            height: 1,
        },
    );
}

/// **In** / **Out** token row: native ticker centred or banner-aligned contract.
#[allow(clippy::too_many_arguments)]
pub fn render_token_field(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    input: &Input,
    focused: bool,
    native_in: bool,
    assets: &[Balance],
    editing: bool,
    screen_width: u16,
    chain_id: u64,
) {
    let inner = brand::render_field_box(frame, area, focused);

    if focused && editing {
        let mut spans = vec![field_label_span(label)];
        spans.extend(input.line().spans);
        frame.render_widget(Paragraph::new(Line::from(spans)), inner);
        return;
    }

    let token_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    if native_in && label == "In" && input.value().trim().is_empty() {
        let sym = native_pls_label(chain_id);
        let style = if focused {
            Style::default()
                .fg(brand::accent_color())
                .add_modifier(Modifier::BOLD)
        } else {
            token_style
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(sym, style))).alignment(Alignment::Center),
            inner,
        );
        frame.render_widget(
            Paragraph::new(Line::from(field_label_span(label))),
            Rect {
                x: inner.x,
                y: inner.y,
                width: (SWAP_LABEL_WIDTH + 1) as u16,
                height: 1,
            },
        );
        return;
    }

    let raw = input.value().trim();
    if raw.is_empty() {
        let mut spans = vec![field_label_span(label)];
        if focused {
            spans.extend(input.line().spans);
        } else {
            spans.push(Span::styled(
                input.placeholder(),
                Style::default().fg(Color::DarkGray),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), inner);
        return;
    }

    let sym = token_symbol_for_address(assets, raw)
        .or_else(|| token_symbol_hint(raw, chain_id))
        .unwrap_or("???");

    frame.render_widget(
        Paragraph::new(brand::colored_token_address_under_augha(
            sym,
            raw,
            screen_width,
            inner.x,
        )),
        inner,
    );
    let label_w = (label.chars().count() + 1) as u16;
    frame.render_widget(
        Paragraph::new(Line::from(field_label_short_span(label))),
        Rect {
            x: inner.x,
            y: inner.y,
            width: label_w,
            height: 1,
        },
    );
}
