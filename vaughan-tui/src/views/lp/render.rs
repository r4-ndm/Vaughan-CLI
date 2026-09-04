#![allow(unused_imports)]
use alloy::primitives::U256;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use std::str::FromStr;
use tokio::runtime::Handle;
use vaughan_core::chains::Balance;
use vaughan_core::core::wiz4rd::WZRD_SMOKE_943;
use vaughan_core::core::{
    build_v2_add_liquidity_evm, build_v2_remove_liquidity_evm, build_v3_collect_evm,
    build_v3_decrease_evm, build_v3_increase_evm, chain_label, default_full_range_ticks,
    display_price_range_from_preset, format_display_amount, lp_stack_for_chain, lp_v3_venue_picker,
    min_out_after_slippage, v3_preview_mint_deposits_from_amount0,
    v3_preview_mint_deposits_from_amount1, v3_range_ticks_from_human_prices,
    v3_sqrt_and_tick_for_preview, venue_position_manager, venue_swap_router, wpls_for_chain,
    DexProtocol, DexVenue, LpStack, V2LpPosition, V3LpDeployWait, V3PoolLifecycle, V3PositionInfo,
    WalletState, DEFAULT_DEX_SLIPPAGE_BPS,
};
use vaughan_core::error::WalletError;
use vaughan_provider::EventBus;

use crate::app::KeyOutcome;
use crate::brand;
use crate::input::{Input, InputAction};
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::swap_form::SWAP_DISPLAY_FRAC;
use crate::views::{
    body_areas, cycle_token_picker, manual_edit_resets_token_pick, render_labeled_input,
    render_labeled_input_aligned, status_paragraph, token_symbol_for_address, TOKEN_PICK_UNINIT,
};
use crate::views::{parse_swap_amount, parse_token_address};

use super::helpers::{
    fee_tier_display, format_unit_price, lp_tx_error_message, parse_price_f64,
    render_unit_price_input, trim_float_string,
};
use super::types::*;

impl LpView {
    pub fn render(&self, frame: &mut Frame, area: Rect, _wallet: &WalletState, assets: &[Balance]) {
        if self.stage == Stage::Confirm {
            let [content, status_area] = body_areas(area);
            self.render_confirm(frame, content);
            let status = if self.busy != Busy::Idle {
                format!("{} {}", spinner_frame(self.tick), self.status)
            } else {
                self.status.clone()
            };
            frame.render_widget(status_paragraph(&status), status_area);
            return;
        }
        if self.tab == Tab::AddLp {
            let [content, status_area] = body_areas(area);
            self.render_add_lp(frame, content, assets);
            let status = if self.busy != Busy::Idle {
                format!("{} {}", spinner_frame(self.tick), self.status)
            } else {
                self.status.clone()
            };
            frame.render_widget(status_paragraph(&status), status_area);
            return;
        }
        self.render_manager(frame, area, assets);
    }

    /// List / Increase / Decrease / Collect / Remove — full-width table, hints at bottom.
    fn render_manager(&self, frame: &mut Frame, area: Rect, assets: &[Balance]) {
        let [header, table, hints, status_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);

        let title = self.engine_title();
        frame.render_widget(
            Paragraph::new(Span::styled(
                title,
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
            header,
        );

        let lines = self.manager_table_lines(assets, table.width);
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(brand::body_color())),
            table,
        );

        frame.render_widget(
            Paragraph::new(Span::styled(
                self.manager_bottom_hints(),
                Style::default().fg(Color::DarkGray),
            )),
            hints,
        );

        let status = if self.busy != Busy::Idle {
            format!("{} {}", spinner_frame(self.tick), self.status)
        } else if !self.status.is_empty()
            && !self.status.starts_with('↑')
            && !self.status.contains("←→ tab")
        {
            self.status.clone()
        } else {
            String::new()
        };
        frame.render_widget(status_paragraph(&status), status_area);
    }

    /// Centered chrome title, e.g. `Wiz4rd-Engine V3`.
    pub(crate) fn engine_title(&self) -> String {
        let stack = match self.stack {
            LpStack::V3 { .. } => "V3",
            LpStack::V2 { .. } => "V2",
        };
        format!("{}-Engine {stack}", self.venue.label())
    }

    fn manager_bottom_hints(&self) -> String {
        if self.tab == Tab::List && self.list_action_idx.is_some() {
            return match self.stack {
                LpStack::V3 { .. } => {
                    "i Increase · d Decrease · c Collect · Esc list · r reload".into()
                }
                LpStack::V2 { .. } => "r Remove · Esc list".into(),
            };
        }
        let tabs = format!("{} · ←→", self.tab_bar());
        let keys = match self.tab {
            Tab::List => "↑↓ select · Enter open · ←→ tabs · r reload · Esc back",
            Tab::Increase => "Enter send · Esc list · r reload",
            Tab::Decrease | Tab::Remove if self.focus == Focus::Liquidity => {
                "type units · Enter · Esc · Tab presets"
            }
            Tab::Decrease | Tab::Remove => "↑↓ % · Tab custom · Enter send · Esc list",
            Tab::Collect => "Enter collect · Esc list · r reload",
            Tab::AddLp => "",
        };
        if keys.is_empty() {
            tabs
        } else {
            format!("{tabs}  ·  {keys}")
        }
    }

    fn manager_table_lines(&self, assets: &[Balance], width: u16) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        match self.tab {
            Tab::List => self.render_list(&mut out, assets, width),
            Tab::Increase => self.render_v3_increase(&mut out, assets, width),
            Tab::Decrease => self.render_v3_decrease(&mut out, assets, width),
            Tab::Collect => self.render_v3_collect(&mut out, assets, width),
            Tab::Remove => self.render_v2_remove(&mut out, assets, width),
            Tab::AddLp => {}
        }
        out
    }

    fn render_confirm(&self, frame: &mut Frame, area: Rect) {
        let title = if self.is_fee_confirm() {
            self.confirm_ui
                .as_ref()
                .map(|ui| match &ui.action {
                    LpConfirmAction::AddReview => " Review add LP ",
                    LpConfirmAction::Enable { .. } => " Enable token ",
                    LpConfirmAction::Deploy { .. } => " Confirm add LP ",
                    LpConfirmAction::Increase => " Confirm increase ",
                    LpConfirmAction::Decrease => " Confirm decrease ",
                    LpConfirmAction::Collect => " Confirm collect ",
                    LpConfirmAction::V2Add => " Confirm add LP ",
                    LpConfirmAction::V2Remove => " Confirm remove ",
                })
                .unwrap_or(" Confirm LP ")
        } else {
            " LP "
        };
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(title)));
        if !self.is_fee_confirm() {
            frame.render_widget(
                Paragraph::new(self.confirm_lines.clone())
                    .wrap(Wrap { trim: true })
                    .style(Style::default().fg(brand::body_color())),
                inner,
            );
            return;
        }

        let mut lines = self
            .confirm_ui
            .as_ref()
            .map(|ui| ui.lines.clone())
            .unwrap_or_else(|| self.confirm_lines.clone());
        let pipeline_step = self.confirm_ui.as_ref().is_some_and(|ui| ui.pipeline_step);
        let is_review = self
            .confirm_ui
            .as_ref()
            .is_some_and(|ui| matches!(ui.action, LpConfirmAction::AddReview));
        let speed = self
            .confirm_ui
            .as_ref()
            .map(|ui| {
                if ui.pipeline_step {
                    self.lp_pipeline_speed
                } else {
                    ui.speed
                }
            })
            .unwrap_or(vaughan_core::chains::FeeSpeed::Normal);
        let confirm_focus = self
            .confirm_ui
            .as_ref()
            .map(|ui| ui.focus)
            .unwrap_or(LpConfirmFocus::Speed);
        let custom_gas = self.confirm_ui.as_ref().map(|ui| &ui.custom_gas);
        let fee = self.selected_fee();
        let fee_total = fee.as_ref().map(|f| f.total.as_str()).unwrap_or("—");
        let fee_detail = fee.as_ref().and_then(|f| match &f.details {
            vaughan_core::chains::FeeDetails::Evm {
                gas_limit,
                max_fee_per_gas,
                ..
            } => {
                let gwei = max_fee_per_gas
                    .as_deref()
                    .and_then(|mf| mf.parse::<u128>().ok())
                    .map(|wei| format!("{:.2} gwei", wei as f64 / 1e9))
                    .unwrap_or_else(|| "—".to_string());
                Some(format!("max {gwei}/gas · limit {gas_limit}"))
            }
            _ => None,
        });
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "Gas (max cap): {fee_total}  [{}]",
            speed.label()
        )));
        if let Some(detail) = fee_detail {
            lines.push(Line::from(format!("            {detail}")));
        }
        if !pipeline_step {
            lines.push(Line::from(""));
            lines.push(Line::from("Gas speed (↑↓ or 1–5):"));
            for (digit, spd) in [
                ('1', vaughan_core::chains::FeeSpeed::Slow),
                ('2', vaughan_core::chains::FeeSpeed::Normal),
                ('3', vaughan_core::chains::FeeSpeed::Fast),
                ('4', vaughan_core::chains::FeeSpeed::Ape),
                ('5', vaughan_core::chains::FeeSpeed::Custom),
            ] {
                let selected = speed == spd;
                let marker = if selected { ">" } else { " " };
                let style = if selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(
                    format!("{marker} {digit} {}", spd.label()),
                    style,
                )));
            }
            if speed == vaughan_core::chains::FeeSpeed::Custom {
                let editing = confirm_focus == LpConfirmFocus::CustomGas;
                let mut spans = vec![Span::raw("    max fee (gwei): ")];
                if editing {
                    if let Some(input) = custom_gas {
                        spans.extend(input.line().spans);
                    }
                } else {
                    let shown = custom_gas
                        .and_then(|input| {
                            if input.value().is_empty() {
                                None
                            } else {
                                Some(input.value())
                            }
                        })
                        .unwrap_or("—");
                    spans.push(Span::styled(
                        shown.to_string(),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                lines.push(Line::from(spans));
            }
        }
        lines.push(Line::from(""));
        let footer = if is_review {
            "Enter continue · Esc cancel"
        } else {
            "Enter send · Esc cancel"
        };
        lines.push(Line::from(Span::styled(
            footer,
            Style::default().fg(brand::accent_color()),
        )));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    }

    pub(crate) fn render_add_lp(&self, frame: &mut Frame, area: Rect, assets: &[Balance]) {
        let on_price_deposit =
            matches!(self.stack, LpStack::V3 { .. }) && self.add_step == AddStep::PriceDeposit;
        let show_fee = matches!(self.stack, LpStack::V3 { .. }) && !on_price_deposit;
        let sym0 = self.token_symbol(&self.token0, assets);
        let sym1 = self.token_symbol(&self.token1, assets);
        let price_suffix = format!(" · {sym1}/{sym0}");

        let mut constraints = vec![
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ];
        if matches!(self.stack, LpStack::V3 { .. }) && !on_price_deposit {
            constraints.push(Constraint::Length(1));
        }
        if !on_price_deposit {
            constraints.push(Constraint::Length(3));
            constraints.push(Constraint::Length(1));
            constraints.push(Constraint::Length(3));
        } else {
            constraints.push(Constraint::Length(1)); // pair · fee summary
            constraints.push(Constraint::Length(1)); // explainer
            constraints.push(Constraint::Length(3)); // range preset box
            if self.v3_custom_range {
                constraints.push(Constraint::Length(4)); // min | current | max | band
            }
            if self.needs_v3_starting_price() && !self.v3_custom_range {
                constraints.push(Constraint::Length(3)); // starting price (new pool)
            }
            constraints.push(Constraint::Length(5)); // summary box (border + 3 lines)
            if !self.v3_custom_range {
                constraints.push(Constraint::Length(1)); // a = adjust hint
            }
            constraints.push(Constraint::Length(1)); // Enable row (PCS)
            constraints.push(Constraint::Length(1)); // deposit title
            constraints.push(Constraint::Length(3)); // deposit row
        }
        if show_fee {
            constraints.push(Constraint::Length(1));
        }
        if on_price_deposit {
            // rows accounted above
        } else if matches!(self.stack, LpStack::V2 { .. }) {
            constraints.extend([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
            ]);
        }
        constraints.push(Constraint::Length(1));
        constraints.push(Constraint::Min(0));

        let chunks = Layout::vertical(constraints).split(area);
        let mut i = 0;

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " Add liquidity ",
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD),
            )))
            .alignment(Alignment::Center),
            chunks[i],
        );
        i += 1;

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Tab ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!("{}   (←→)", self.tab_bar())),
            ]))
            .alignment(Alignment::Center),
            chunks[i],
        );
        i += 1;

        let step_label = if on_price_deposit {
            "Step 2/2 — Pick a range preset (a to fine-tune) · then deposit"
        } else if matches!(self.stack, LpStack::V3 { .. }) {
            "Step 1/2 — Pick the two tokens and fee tier"
        } else {
            "Select tokens, ratio, and deposit"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                step_label,
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Center),
            chunks[i],
        );
        i += 1;

        if matches!(self.stack, LpStack::V3 { .. }) && !on_price_deposit {
            let venue_style = if self.focus == Focus::Venue {
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(brand::body_color())
            };
            let picker: Vec<_> = lp_v3_venue_picker(self.chain_id)
                .iter()
                .map(|v| {
                    let on_chain = venue_position_manager(*v, self.chain_id).is_some();
                    let mark = if *v == self.venue { "[" } else { "" };
                    let end = if *v == self.venue { "]" } else { "" };
                    if on_chain {
                        format!("{mark}{}{end}", v.label())
                    } else {
                        format!("{mark}{}(943){end}", v.label())
                    }
                })
                .collect();
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("{} · ↑↓ pick", picker.join(" · ")),
                    venue_style,
                )))
                .alignment(Alignment::Center),
                chunks[i],
            );
            i += 1;
        }

        if !on_price_deposit {
            self.render_token_field(
                frame,
                chunks[i],
                "First token",
                &self.token0,
                self.focus == Focus::Token0,
                assets,
                self.token0_editing,
                area.width,
            );
            i += 1;
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "+",
                    Style::default().fg(Color::DarkGray),
                )))
                .alignment(Alignment::Center),
                chunks[i],
            );
            i += 1;
            self.render_token_field(
                frame,
                chunks[i],
                "Second token",
                &self.token1,
                self.focus == Focus::Token1,
                assets,
                self.token1_editing,
                area.width,
            );
            i += 1;
        } else {
            frame.render_widget(
                Paragraph::new(Line::from(format!(
                    "{} · {} + {} · fee {}",
                    self.venue.label(),
                    sym0,
                    sym1,
                    fee_tier_display(self.fee_tier)
                )))
                .alignment(Alignment::Center),
                chunks[i],
            );
            i += 1;
        }

        if show_fee {
            frame.render_widget(
                Paragraph::new(self.fee_tier_line()).alignment(Alignment::Center),
                chunks[i],
            );
            i += 1;
        }

        if on_price_deposit {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    self.simple_range_explainer(),
                    Style::default().fg(Color::DarkGray),
                )))
                .wrap(Wrap { trim: true })
                .alignment(Alignment::Center),
                chunks[i],
            );
            i += 1;
            self.render_range_preset_row(frame, chunks[i]);
            i += 1;
            if self.v3_custom_range {
                self.render_price_range_columns(frame, chunks[i], sym0, sym1, &price_suffix);
                i += 1;
            }
            if self.needs_v3_starting_price() && !self.v3_custom_range {
                render_unit_price_input(
                    frame,
                    chunks[i],
                    &format!("Starting price (new pool) · 1 {sym0} ="),
                    sym1,
                    &self.initial_price,
                    self.focus == Focus::InitialPrice,
                    Alignment::Left,
                );
                i += 1;
            }
            self.render_simple_range_summary(frame, chunks[i], sym0, sym1);
            i += 1;
            if !self.v3_custom_range {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "a = fine-tune min / current / max later",
                        Style::default().fg(Color::DarkGray),
                    )))
                    .alignment(Alignment::Center),
                    chunks[i],
                );
                i += 1;
            }
            if let Some(line) = self.enable_status_line(sym0, sym1) {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        line,
                        Style::default().fg(brand::accent_color()),
                    )))
                    .alignment(Alignment::Center),
                    chunks[i],
                );
            }
            i += 1;
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "How much to add?",
                    Style::default()
                        .fg(brand::accent_color())
                        .add_modifier(Modifier::BOLD),
                )))
                .alignment(Alignment::Center),
                chunks[i],
            );
            i += 1;
            self.render_deposit_columns(frame, chunks[i], sym0, sym1);
            i += 1;
        } else if matches!(self.stack, LpStack::V2 { .. }) {
            render_labeled_input(
                frame,
                chunks[i],
                &format!("Ratio{price_suffix}"),
                &self.ratio,
                self.focus == Focus::Ratio,
            );
            i += 1;
            render_labeled_input(
                frame,
                chunks[i],
                &format!("Deposit {sym0}"),
                &self.amount0,
                self.focus == Focus::Amount0,
            );
            i += 1;
            render_labeled_input(
                frame,
                chunks[i],
                &format!("Deposit {sym1}"),
                &self.amount1,
                self.focus == Focus::Amount1,
            );
            i += 1;
        }

        let hint = if on_price_deposit {
            if self.v3_custom_range {
                "Tab fields · ←→ range · e enable · Enter add · a presets-only · Esc back"
            } else {
                "Tab fields · ←→ range · e enable · Enter add · a fine-tune · Esc back"
            }
        } else if matches!(self.stack, LpStack::V3 { .. }) {
            "Tab · ↑↓ tokens/venue · ←→ fee · Enter continue"
        } else {
            "Tab · field · ↑↓ token · Enter · add liquidity"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hint,
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Center),
            chunks[i],
        );
    }

    pub(crate) fn fee_tier_line(&self) -> Line<'static> {
        let mut spans = vec![Span::raw("Fee tier: ")];
        for &tier in LP_FEE_TIERS {
            let label = fee_tier_display(tier);
            let selected = tier == self.fee_tier;
            let style = if selected {
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else if self.focus == Focus::Fee {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Span::styled(format!(" {label} "), style));
        }
        if self.focus == Focus::Fee {
            spans.push(Span::styled(
                " ←→",
                Style::default().fg(brand::accent_color()),
            ));
        }
        Line::from(spans)
    }

    pub(crate) fn render_range_preset_row(&self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::RangePresets;
        let title = if focused {
            brand::focus_title(" Range width ")
        } else {
            brand::fade_line(" Range width ")
        };
        let inner = brand::render_labeled_input_box(frame, area, Some(title), focused);
        frame.render_widget(
            Paragraph::new(self.range_preset_line()).alignment(Alignment::Center),
            inner,
        );
    }

    pub(crate) fn range_preset_line(&self) -> Line<'static> {
        let mut spans = Vec::with_capacity(RANGE_PRESETS.len() * 2);
        for (i, (label, _)) in RANGE_PRESETS.iter().enumerate() {
            let applied = self.range_preset_applied == Some(i);
            let highlighted = self.focus == Focus::RangePresets && self.range_preset_idx == i;
            let style = if applied {
                Style::default()
                    .fg(brand::accent_color())
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else if highlighted {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Span::styled(format!(" {label} "), style));
        }
        if self.focus == Focus::RangePresets {
            spans.push(Span::styled(
                " ←→ pick · Enter next",
                Style::default().fg(brand::accent_color()),
            ));
        }
        Line::from(spans)
    }

    pub(crate) fn render_price_range_columns(
        &self,
        frame: &mut Frame,
        area: Rect,
        sym0: &str,
        sym1: &str,
        price_suffix: &str,
    ) {
        let cols = Layout::horizontal([
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
        ])
        .split(area);
        render_labeled_input_aligned(
            frame,
            cols[0],
            &format!("Min{price_suffix}"),
            &self.min_price,
            self.focus == Focus::MinPrice,
            Alignment::Center,
        );
        render_unit_price_input(
            frame,
            cols[1],
            &format!("Current · 1 {sym0} ="),
            sym1,
            &self.initial_price,
            self.focus == Focus::InitialPrice,
            Alignment::Center,
        );
        render_labeled_input_aligned(
            frame,
            cols[2],
            &format!("Max{price_suffix}"),
            &self.max_price,
            self.focus == Focus::MaxPrice,
            Alignment::Center,
        );
        self.render_range_summary_cell(frame, cols[3], sym0, sym1);
    }

    pub(crate) fn render_range_summary_cell(
        &self,
        frame: &mut Frame,
        area: Rect,
        sym0: &str,
        sym1: &str,
    ) {
        let title = brand::fade_line(" Range ");
        let inner = brand::render_faded_box(frame, area, Some(title));
        let inv = self
            .center_price_f64()
            .map(|p| trim_float_string(1.0 / p))
            .unwrap_or_else(|| "—".to_string());
        let band = self.range_band_label();
        let lines = vec![
            Line::from(vec![
                Span::styled("Inv ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{inv} {sym0}/{sym1}")),
            ]),
            Line::from(Span::styled(band, Style::default().fg(brand::body_color()))),
        ];
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
    }

    pub(crate) fn render_deposit_columns(
        &self,
        frame: &mut Frame,
        area: Rect,
        sym0: &str,
        sym1: &str,
    ) {
        let cols =
            Layout::horizontal([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)]).split(area);
        render_labeled_input(
            frame,
            cols[0],
            &format!("Deposit {sym0}"),
            &self.amount0,
            self.focus == Focus::Amount0,
        );
        render_labeled_input(
            frame,
            cols[1],
            &format!("Deposit {sym1}"),
            &self.amount1,
            self.focus == Focus::Amount1,
        );
    }

    pub(crate) fn field_label_span(label: &str) -> Span<'static> {
        Span::styled(
            format!("{label}: "),
            Style::default().add_modifier(Modifier::BOLD),
        )
    }

    pub(crate) fn field_label_short_span(label: &str) -> Span<'static> {
        Span::styled(
            format!("{label}: "),
            Style::default().add_modifier(Modifier::BOLD),
        )
    }

    pub(crate) fn token_symbol<'a>(&self, input: &Input, assets: &'a [Balance]) -> &'a str {
        let raw = input.value().trim();
        token_symbol_for_address(assets, raw)
            .or_else(|| crate::views::token_symbol_hint(raw, self.chain_id))
            .unwrap_or("???")
    }

    /// Token picker box (First / Second token).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_token_field(
        &self,
        frame: &mut Frame,
        area: Rect,
        label: &str,
        input: &Input,
        focused: bool,
        assets: &[Balance],
        editing: bool,
        screen_width: u16,
    ) {
        let inner = brand::render_field_box(frame, area, focused);

        if focused && editing {
            let mut spans = vec![Self::field_label_span(label)];
            spans.extend(input.line().spans);
            frame.render_widget(Paragraph::new(Line::from(spans)), inner);
            return;
        }

        let raw = input.value().trim();
        if raw.is_empty() {
            let mut spans = vec![Self::field_label_span(label)];
            if focused {
                spans.extend(input.line().spans);
            } else {
                spans.push(Span::styled("Select", Style::default().fg(Color::DarkGray)));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), inner);
            return;
        }

        let sym = self.token_symbol(input, assets);
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
            Paragraph::new(Line::from(Self::field_label_short_span(label))),
            Rect {
                x: inner.x,
                y: inner.y,
                width: label_w.min(inner.width),
                height: 1,
            },
        );
    }

    pub(crate) fn tab_bar(&self) -> String {
        match self.stack {
            LpStack::V3 { .. } => Tab::v3_cycle()
                .iter()
                .map(|t| {
                    if *t == self.tab {
                        format!("[{}]", t.label())
                    } else {
                        t.label().to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" · "),
            LpStack::V2 { .. } => Tab::v2_cycle()
                .iter()
                .map(|t| {
                    if *t == self.tab {
                        format!("[{}]", t.label())
                    } else {
                        t.label().to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" · "),
        }
    }

    pub(crate) fn render_list(
        &self,
        out: &mut Vec<Line<'static>>,
        assets: &[Balance],
        width: u16,
    ) {
        if !self.lp_supported() {
            out.push(Line::from("(LP not available on this network)"));
            return;
        }
        // Focused position: one row + actions live on the bottom hint bar (no under-row ticks).
        if self.list_action_idx.is_some() {
            self.render_focused_position(out, assets, width);
            return;
        }
        match self.stack {
            LpStack::V3 { .. } => {
                if self.v3_positions.is_empty() {
                    out.push(Line::from("(no positions — Add LP or r reload)"));
                } else {
                    out.push(super::helpers::v3_table_header_line(width));
                    for (i, p) in self.v3_positions.iter().enumerate() {
                        out.push(super::helpers::v3_table_row_line(
                            self.chain_id,
                            p,
                            assets,
                            i == self.sel,
                            width,
                        ));
                    }
                }
            }
            LpStack::V2 { .. } => {
                if self.v2_positions.is_empty() {
                    out.push(Line::from("(no positions — Add LP or r reload)"));
                } else {
                    out.push(super::helpers::v2_table_header_line(width));
                    for (i, p) in self.v2_positions.iter().enumerate() {
                        out.push(super::helpers::v2_table_row_line(
                            self.chain_id,
                            p,
                            assets,
                            i == self.sel,
                            width,
                        ));
                    }
                }
            }
        }
    }

    fn render_focused_position(
        &self,
        out: &mut Vec<Line<'static>>,
        assets: &[Balance],
        _width: u16,
    ) {
        match self.stack {
            LpStack::V3 { .. } => {
                let Some(p) = self.v3_positions.get(self.sel) else {
                    out.push(Line::from("No position selected"));
                    return;
                };
                for line in super::helpers::v3_focused_detail_lines(
                    self.chain_id,
                    self.venue.label(),
                    p,
                    assets,
                ) {
                    out.push(line);
                }
                if let Some(hint) =
                    super::helpers::v3_manage_hint(p.liquidity, p.tokens_owed0, p.tokens_owed1)
                {
                    out.push(Line::from(""));
                    out.push(Line::from(Span::styled(
                        format!("  {hint}"),
                        Style::default().fg(Color::Yellow),
                    )));
                }
            }
            LpStack::V2 { .. } => {
                let Some(p) = self.v2_positions.get(self.sel) else {
                    out.push(Line::from("No position selected"));
                    return;
                };
                for line in super::helpers::v2_focused_detail_lines(
                    self.chain_id,
                    self.venue.label(),
                    p,
                    assets,
                ) {
                    out.push(line);
                }
            }
        }
    }

    /// Compact selected-row table shared by Increase / Decrease / Collect.
    fn push_selected_v3_table(
        &self,
        out: &mut Vec<Line<'static>>,
        assets: &[Balance],
        width: u16,
    ) -> bool {
        let Some(p) = self.v3_positions.get(self.sel) else {
            out.push(Line::from("Select a position on List first"));
            return false;
        };
        out.push(super::helpers::v3_table_header_line(width));
        out.push(super::helpers::v3_table_row_line(
            self.chain_id,
            p,
            assets,
            true,
            width,
        ));
        true
    }

    pub(crate) fn render_v3_increase(
        &self,
        out: &mut Vec<Line<'static>>,
        assets: &[Balance],
        width: u16,
    ) {
        if !self.push_selected_v3_table(out, assets, width) {
            return;
        }
        out.push(Line::from(format!(
            "amt0 {} · amt1 {}",
            self.amount0.value(),
            self.amount1.value()
        )));
    }

    pub(crate) fn decrease_preset_line(&self) -> Line<'static> {
        let mut spans = vec![Span::raw("Remove ")];
        for (i, (label, _)) in super::types::DECREASE_PRESETS.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" · "));
            }
            let applied = self.decrease_preset_applied == Some(i);
            let style = if applied {
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(brand::accent_color())
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Span::styled(*label, style));
        }
        Line::from(spans)
    }

    fn decrease_amount_summary(&self, total: alloy::primitives::U256) -> String {
        let remove = alloy::primitives::U256::from_str(self.liquidity.value().trim())
            .unwrap_or(alloy::primitives::U256::ZERO);
        let keep = total.saturating_sub(remove);
        let pct = if total.is_zero() {
            0u64
        } else {
            let raw = (remove * alloy::primitives::U256::from(100u64)) / total;
            raw.min(alloy::primitives::U256::from(100u64))
                .to::<u64>()
        };
        format!(
            "−{}%  {} → keep {}",
            pct,
            super::helpers::short_units(remove),
            super::helpers::short_units(keep),
        )
    }

    pub(crate) fn render_v3_decrease(
        &self,
        out: &mut Vec<Line<'static>>,
        assets: &[Balance],
        width: u16,
    ) {
        if !self.push_selected_v3_table(out, assets, width) {
            return;
        }
        if let Some(p) = self.v3_positions.get(self.sel) {
            if let Some(hint) =
                super::helpers::v3_manage_hint(p.liquidity, p.tokens_owed0, p.tokens_owed1)
            {
                out.push(Line::from(Span::styled(
                    hint,
                    Style::default().fg(Color::Yellow),
                )));
            }
            out.push(self.decrease_preset_line());
            out.push(Line::from(self.decrease_amount_summary(
                alloy::primitives::U256::from(p.liquidity),
            )));
            if self.focus == Focus::Liquidity {
                out.push(Line::from(format!(
                    "> remove units: {}",
                    self.liquidity.value()
                )));
            }
        }
    }

    pub(crate) fn render_v3_collect(
        &self,
        out: &mut Vec<Line<'static>>,
        assets: &[Balance],
        width: u16,
    ) {
        if !self.push_selected_v3_table(out, assets, width) {
            return;
        }
        if let Some(p) = self.v3_positions.get(self.sel) {
            if p.tokens_owed0 > 0 || p.tokens_owed1 > 0 {
                out.push(Line::from(Span::styled(
                    "Fees/tokens owed ready",
                    Style::default().fg(brand::accent_color()),
                )));
            } else {
                out.push(Line::from(Span::styled(
                    "No tokens owed right now",
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }

    pub(crate) fn render_v2_remove(
        &self,
        out: &mut Vec<Line<'static>>,
        assets: &[Balance],
        width: u16,
    ) {
        let Some(p) = self.v2_positions.get(self.sel) else {
            out.push(Line::from("Select a position on List first"));
            return;
        };
        out.push(super::helpers::v2_table_header_line(width));
        out.push(super::helpers::v2_table_row_line(
            self.chain_id,
            p,
            assets,
            true,
            width,
        ));
        out.push(self.decrease_preset_line());
        out.push(Line::from(self.decrease_amount_summary(p.lp_balance)));
        if self.focus == Focus::Liquidity {
            out.push(Line::from(format!(
                "> remove LP: {}",
                self.liquidity.value()
            )));
        }
    }
}
