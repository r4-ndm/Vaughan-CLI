//! Rendering for the `vaughan-hl` TUI: header, hyper-chart overlay, strategy
//! details, live trade tape, quick-connect, and footer. Lifted out of
//! `tui/mod.rs` as a second `impl HlTuiApp` block — pure drawing (`&self`,
//! `&mut Frame`, `Rect`), no state mutation. `pub(super)` since `tui/mod.rs`'s
//! top-level `draw` dispatches into these.

use super::*;

impl HlTuiApp {
    /// Render interactive candlestick chart overlay with plotted trade entries, exits, and PnL.
    pub(super) fn draw_hyper_chart(&self, frame: &mut Frame, area: Rect) {
        let w = area.width.saturating_sub(4).max(50);
        let h = area.height.saturating_sub(2).max(18);
        let popup = Rect::new(
            area.x + area.width.saturating_sub(w) / 2,
            area.y + area.height.saturating_sub(h) / 2,
            w,
            h,
        );
        frame.render_widget(Clear, popup);

        let Some(cur) = self.current() else {
            return;
        };

        // Determine available markets for this strategy
        let markets = if !cur.selected_markets.is_empty() {
            &cur.selected_markets
        } else {
            std::slice::from_ref(&cur.compiled.config.market.coin)
        };
        let coin = markets
            .get(self.chart_market_idx % markets.len())
            .unwrap_or(&cur.compiled.config.market.coin);
        let interval = &cur.compiled.config.market.interval;

        let trades: &Vec<SimulatedTrade> = if coin == &cur.compiled.config.market.coin {
            &cur.report.trades
        } else if let Some(other) = self
            .entries
            .iter()
            .find(|e| e.report.coin == *coin && e.report.interval == *interval)
        {
            &other.report.trades
        } else {
            &cur.report.trades
        };

        let candles_opt = load_cached_or_derived(&self.data_dir, coin, interval);

        let title = format!(
            " 📈 Hyper Chart: {} [{}] · Strategy: {} · {} Trades · Press [c/Esc] to Close ",
            coin,
            interval,
            cur.file_name
                .strip_suffix(".toml")
                .unwrap_or(&cur.file_name),
            trades.len()
        );
        let block = Block::default()
            .title(Span::styled(
                title,
                Style::default()
                    .fg(COL_HOT_PINK)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COL_NEON_BLUE));

        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let Some(candles) = candles_opt.filter(|c| !c.is_empty()) else {
            let msg = format!(
                "\n  No historical candles cached for {} [{}].\n  Run 'vaughan fetch --coin {} --interval {}' to download candle history.",
                coin, interval, coin, interval
            );
            frame.render_widget(
                Paragraph::new(msg).style(Style::default().fg(COL_YELLOW)),
                inner,
            );
            return;
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // Top stats ribbon
                Constraint::Min(8),    // The Candlestick & Trade Chart
                Constraint::Length(6), // Plotted Trades Table
                Constraint::Length(1), // Footer hotkey hints
            ])
            .split(inner);

        let cw = chunks[1].width as usize;
        let ch = chunks[1].height as usize;
        if cw < 20 || ch < 4 {
            return;
        }

        let price_col_w = 13.min(cw.saturating_sub(10));
        let candle_area_w = cw.saturating_sub(price_col_w);

        let slot_w = 2; // 2 characters per candle
        let max_visible = candle_area_w / slot_w;
        let total_candles = candles.len();
        let max_scroll = total_candles.saturating_sub(max_visible);
        let scroll = self.chart_scroll.min(max_scroll);
        let end_idx = total_candles.saturating_sub(scroll);
        let start_idx = end_idx.saturating_sub(max_visible);
        let visible_candles = &candles[start_idx..end_idx];

        let mut high_max = f64::NEG_INFINITY;
        let mut low_min = f64::INFINITY;
        for c in visible_candles {
            if c.high > high_max {
                high_max = c.high;
            }
            if c.low < low_min {
                low_min = c.low;
            }
        }
        if high_max <= low_min || !high_max.is_finite() {
            high_max = low_min + 1.0;
        }
        let span = high_max - low_min;
        let pad = (span * 0.04).max(0.0001);
        high_max += pad;
        low_min -= pad;
        let price_range = high_max - low_min;

        let to_y = |px: f64| -> usize {
            let frac = (high_max - px) / price_range;
            (frac * (ch.saturating_sub(1)) as f64)
                .round()
                .clamp(0.0, (ch.saturating_sub(1)) as f64) as usize
        };

        struct CandlePlot {
            y_high: usize,
            y_low: usize,
            body_top: usize,
            body_bot: usize,
            is_green: bool,
            entry: Option<Side>,
            exit_win: Option<bool>,
        }

        let mut plots: Vec<CandlePlot> = Vec::with_capacity(visible_candles.len());
        for c in visible_candles {
            let mut en = None;
            let mut ex = None;
            for t in trades {
                if t.entry_time_ms >= c.open_time_ms && t.entry_time_ms <= c.close_time_ms {
                    en = Some(t.side);
                }
                if t.exit_time_ms >= c.open_time_ms && t.exit_time_ms <= c.close_time_ms {
                    ex = Some(t.realized_r > 0.0);
                }
            }

            let yh = to_y(c.high);
            let yl = to_y(c.low);
            let yo = to_y(c.open);
            let yc = to_y(c.close);
            plots.push(CandlePlot {
                y_high: yh,
                y_low: yl,
                body_top: yo.min(yc),
                body_bot: yo.max(yc),
                is_green: c.close >= c.open,
                entry: en,
                exit_win: ex,
            });
        }

        let mut chart_lines: Vec<Line> = Vec::with_capacity(ch);
        for r in 0..ch {
            let grid_row = r % 4 == 0 || r == 0 || r == ch - 1;
            let mut spans: Vec<Span> = Vec::new();

            for cy in &plots {
                let candle_color = if cy.is_green { COL_GREEN } else { COL_RED };

                // Char 1: Candle Body / Wick / Entry Marker
                if cy.entry == Some(Side::Long)
                    && (r == cy.y_low.saturating_add(1) || (cy.y_low == ch - 1 && r == cy.y_low))
                {
                    spans.push(Span::styled(
                        "▲",
                        Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                    ));
                } else if cy.entry == Some(Side::Short)
                    && (r == cy.y_high.saturating_sub(1) || (cy.y_high == 0 && r == 0))
                {
                    spans.push(Span::styled(
                        "▼",
                        Style::default()
                            .fg(COL_HOT_PINK)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else if r >= cy.y_high && r < cy.body_top {
                    spans.push(Span::styled("│", Style::default().fg(candle_color)));
                } else if r >= cy.body_top && r <= cy.body_bot {
                    spans.push(Span::styled(
                        "█",
                        Style::default()
                            .fg(candle_color)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else if r > cy.body_bot && r <= cy.y_low {
                    spans.push(Span::styled("│", Style::default().fg(candle_color)));
                } else if grid_row {
                    spans.push(Span::styled(
                        "┄",
                        Style::default().fg(Color::Rgb(40, 50, 70)),
                    ));
                } else {
                    spans.push(Span::raw(" "));
                }

                // Char 2: Exit marker or spacing
                if let Some(win) = cy.exit_win {
                    if r == cy.y_high || r == cy.body_top {
                        if win {
                            spans.push(Span::styled(
                                "✓",
                                Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                            ));
                        } else {
                            spans.push(Span::styled(
                                "✕",
                                Style::default().fg(COL_RED).add_modifier(Modifier::BOLD),
                            ));
                        }
                    } else if grid_row {
                        spans.push(Span::styled(
                            "┄",
                            Style::default().fg(Color::Rgb(40, 50, 70)),
                        ));
                    } else {
                        spans.push(Span::raw(" "));
                    }
                } else if grid_row {
                    spans.push(Span::styled(
                        "┄",
                        Style::default().fg(Color::Rgb(40, 50, 70)),
                    ));
                } else {
                    spans.push(Span::raw(" "));
                }
            }

            // Fill empty horizontal space before price axis
            let used_w = plots.len() * slot_w;
            if candle_area_w > used_w {
                let fill_str = if grid_row {
                    "┄".repeat(candle_area_w - used_w)
                } else {
                    " ".repeat(candle_area_w - used_w)
                };
                let style = if grid_row {
                    Style::default().fg(Color::Rgb(40, 50, 70))
                } else {
                    Style::default()
                };
                spans.push(Span::styled(fill_str, style));
            }

            // Price Scale Label on the right
            let price_at_r =
                high_max - (r as f64 / (ch.saturating_sub(1).max(1)) as f64) * price_range;
            let price_label = if grid_row {
                if price_at_r >= 1000.0 {
                    format!("┤ ${:<8.2}", price_at_r)
                } else if price_at_r >= 1.0 {
                    format!("┤ ${:<8.4}", price_at_r)
                } else {
                    format!("┤ ${:<8.6}", price_at_r)
                }
            } else {
                "│         ".to_string()
            };
            spans.push(Span::styled(
                price_label,
                if grid_row {
                    Style::default().fg(COL_DIM)
                } else {
                    Style::default().fg(Color::Rgb(40, 50, 70))
                },
            ));

            chart_lines.push(Line::from(spans));
        }

        frame.render_widget(Paragraph::new(chart_lines), chunks[1]);

        // Top Stats Ribbon
        let latest_px = candles.last().map_or(0.0, |c| c.close);
        let visible_start_t = visible_candles.first().map_or(0, |c| c.open_time_ms);
        let visible_end_t = visible_candles.last().map_or(0, |c| c.close_time_ms);
        let trades_in_view: Vec<&SimulatedTrade> = trades
            .iter()
            .filter(|t| t.entry_time_ms >= visible_start_t && t.entry_time_ms <= visible_end_t)
            .collect();
        let view_net_r: f64 = trades_in_view.iter().map(|t| t.realized_r).sum();

        let ribbon = Line::from(vec![
            Span::styled(
                format!("  {} [{}] ", coin, interval),
                Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("Latest: ${:.2}  │ ", latest_px),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                format!("Range: ${:.2} - ${:.2}  │ ", low_min, high_max),
                Style::default().fg(COL_DIM),
            ),
            Span::styled(
                format!(
                    "Visible: {}/{} bars  │ ",
                    visible_candles.len(),
                    total_candles
                ),
                Style::default().fg(COL_DIM),
            ),
            Span::styled(
                format!(
                    "Plotted Trades: {} (Net: {:>+5.2}R)  ▲ Long: {}  ▼ Short: {}",
                    trades_in_view.len(),
                    view_net_r,
                    trades_in_view
                        .iter()
                        .filter(|t| t.side == Side::Long)
                        .count(),
                    trades_in_view
                        .iter()
                        .filter(|t| t.side == Side::Short)
                        .count()
                ),
                Style::default()
                    .fg(if view_net_r >= 0.0 {
                        COL_GREEN
                    } else {
                        COL_RED
                    })
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        let ribbon_sub = Line::from(vec![Span::styled(
            format!(
                "  Markets: {} (Press [Tab] or [m] to switch)  │  Scroll Offset: -{} bars",
                markets.join(", "),
                scroll
            ),
            Style::default().fg(COL_DIM),
        )]);
        frame.render_widget(Paragraph::new(vec![ribbon, ribbon_sub]), chunks[0]);

        // Plotted Trades Table
        let trade_rows: Vec<Row> = trades_in_view
            .iter()
            .rev()
            .take(4)
            .map(|t| {
                let win = t.realized_r > 0.0;
                let outcome_color = if win { COL_GREEN } else { COL_RED };
                let side_str = match t.side {
                    Side::Long => "▲ LONG",
                    Side::Short => "▼ SHORT",
                };
                Row::new(vec![
                    format!("#{}", t.id),
                    side_str.to_string(),
                    format!("${:.2}", t.entry_px),
                    format!("${:.2}", t.exit_px),
                    format!("{:>+5.2}R", t.realized_r),
                    format!("${:>+6.2}", t.net_pnl_usd),
                    t.exit_reason.as_str().to_string(),
                ])
                .style(Style::default().fg(outcome_color))
            })
            .collect();

        if trade_rows.is_empty() {
            let no_tr = Paragraph::new("  ● No trade executions in this visible window. Use [← / →] or [PgUp / PgDn] to scroll through candle history.")
                .style(Style::default().fg(COL_DIM));
            frame.render_widget(no_tr, chunks[2]);
        } else {
            let header = Row::new(vec![
                "#",
                "SIDE",
                "ENTRY PX",
                "EXIT PX",
                "NET R",
                "NET USD",
                "EXIT REASON",
            ])
            .style(Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD));
            let table = Table::new(
                trade_rows,
                [
                    Constraint::Length(5),
                    Constraint::Length(9),
                    Constraint::Length(14),
                    Constraint::Length(14),
                    Constraint::Length(9),
                    Constraint::Length(11),
                    Constraint::Min(16),
                ],
            )
            .header(header)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(COL_BORDER)),
            );
            frame.render_widget(table, chunks[2]);
        }

        // Hotkey hints footer
        let footer_line = Line::from(vec![
            Span::styled(
                " [c / Esc / q]",
                Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Close Chart  │ "),
            Span::styled(
                " [← / →]",
                Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Scroll (5 bars)  │ "),
            Span::styled(
                " [PgUp / PgDn]",
                Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Fast Scroll (25)  │ "),
            Span::styled(
                " [Home / End]",
                Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" First / Latest  │ "),
            Span::styled(
                " [Tab / m]",
                Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Next Market "),
        ]);
        frame.render_widget(Paragraph::new(footer_line), chunks[3]);
    }

    pub(super) fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let is_wide = area.width >= 115;
        let is_compact = area.width < 90;

        let mode_span = if self.live_on {
            Span::styled(
                " ● LIVE ",
                Style::default()
                    .fg(Color::White)
                    .bg(COL_RED)
                    .add_modifier(Modifier::BOLD),
            )
        } else if self.armed_dry_run {
            Span::styled(
                " ● PAPER ",
                Style::default()
                    .fg(Color::Black)
                    .bg(COL_GREEN)
                    .add_modifier(Modifier::BOLD),
            )
        } else if live::active_runs().iter().any(|h| h.snapshot().halted) {
            Span::styled(
                " ● HALTED ",
                Style::default()
                    .fg(Color::White)
                    .bg(COL_RED)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                " ○ STANDBY ",
                Style::default()
                    .fg(Color::Black)
                    .bg(COL_YELLOW)
                    .add_modifier(Modifier::BOLD),
            )
        };

        let net_badge = if self.is_mainnet {
            Span::styled(
                "[MAINNET]",
                Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                "[TESTNET]",
                Style::default().fg(COL_YELLOW).add_modifier(Modifier::BOLD),
            )
        };

        let telem_opt = self.live_telemetry_data.as_ref();
        let portfolio_val = telem_opt
            .map(|t| t.portfolio_value)
            .or(self.live_equity_usd)
            .unwrap_or_else(|| self.live_spot_balance_usd.unwrap_or(0.0));
        let avail_val = telem_opt.map(|t| t.available_usdc).unwrap_or(portfolio_val);
        let upnl = telem_opt.map(|t| t.total_unrealized_pnl).unwrap_or(0.0);
        let upnl_sign = if upnl >= 0.0 { "+" } else { "" };
        let upnl_col = if upnl >= 0.0 { COL_GREEN } else { COL_RED };

        let balance_spans = if self.is_connected {
            if portfolio_val > 0.0 {
                if is_compact {
                    vec![
                        Span::styled(
                            format!("${portfolio_val:.2}"),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" ({upnl_sign}${upnl:.2})"),
                            Style::default().fg(upnl_col).add_modifier(Modifier::BOLD),
                        ),
                    ]
                } else {
                    vec![
                        Span::styled("Equity: ", Style::default().fg(COL_DIM)),
                        Span::styled(
                            format!("${portfolio_val:.2}"),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" ({upnl_sign}${upnl:.2})"),
                            Style::default().fg(upnl_col).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(" · ", Style::default().fg(COL_DIM)),
                        Span::styled(
                            format!("${avail_val:.2} Avail"),
                            Style::default().fg(COL_GREEN),
                        ),
                    ]
                }
            } else {
                vec![
                    Span::styled("Equity: ", Style::default().fg(COL_DIM)),
                    Span::styled("$0.00 USDC", Style::default().fg(COL_GREEN)),
                ]
            }
        } else {
            vec![Span::styled("○ Disconnected", Style::default().fg(COL_DIM))]
        };

        let wallet_spans: Vec<Span> = if self.copy_pulse_ticks > 0 {
            let glyph = spinner_frame(self.anim_tick);
            vec![
                Span::styled("Agent: ", Style::default().fg(COL_DIM)),
                Span::styled(
                    self.agent_address
                        .map(|a| format_short_addr(&a))
                        .unwrap_or_else(|| "0x...".to_string()),
                    Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {glyph} ✓ Copied!"),
                    Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                ),
            ]
        } else if let (Some(agent), Some(backing)) = (self.agent_address, self.backing_account) {
            if is_wide {
                vec![
                    Span::styled("Agent: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format_short_addr(&agent),
                        Style::default()
                            .fg(COL_NEON_BLUE)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" -> ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format_short_addr(&backing),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" [y:copy]", Style::default().fg(COL_KEY)),
                ]
            } else {
                vec![
                    Span::styled("Agent: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format_short_addr(&agent),
                        Style::default()
                            .fg(COL_NEON_BLUE)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" [y]", Style::default().fg(COL_KEY)),
                ]
            }
        } else if let Some(agent) = self.agent_address {
            vec![
                Span::styled("Agent: ", Style::default().fg(COL_DIM)),
                Span::styled(
                    format_short_addr(&agent),
                    Style::default()
                        .fg(COL_NEON_BLUE)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" [y:copy]", Style::default().fg(COL_KEY)),
            ]
        } else if self.is_connected {
            vec![Span::styled(
                "Agent Connected",
                Style::default().fg(COL_GREEN),
            )]
        } else {
            vec![Span::styled(
                "No Agent [Enter: connect]",
                Style::default().fg(COL_DIM),
            )]
        };

        let mut header_spans = vec![
            Span::styled(
                " VAUGHAN",
                Style::default()
                    .fg(COL_NEON_BLUE)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "-HL",
                Style::default()
                    .fg(COL_HOT_PINK)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default()),
            mode_span,
            Span::styled("  ", Style::default()),
            net_badge,
            Span::styled("  │  ", Style::default().fg(COL_DIM)),
        ];

        header_spans.extend(balance_spans);
        header_spans.push(Span::styled("  │  ", Style::default().fg(COL_DIM)));
        header_spans.extend(wallet_spans);

        let header_line = Line::from(header_spans);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COL_BORDER));
        frame.render_widget(Paragraph::new(header_line).block(block), area);
    }

    pub(super) fn draw_strategy_details(&self, frame: &mut Frame, area: Rect) {
        let title_text = if area.width < 75 {
            " Live Account & Strategy "
        } else {
            " Live Account & Trading Telemetry "
        };

        let block = Block::default()
            .title(Span::styled(
                title_text,
                Style::default()
                    .fg(COL_NEON_BLUE)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COL_BORDER));

        let Some(cur) = self.current() else {
            frame.render_widget(Paragraph::new("No strategies loaded.").block(block), area);
            return;
        };

        let is_narrow = area.width < 75;

        let telem_opt = self.live_telemetry_data.as_ref();
        let live_eq = self.live_equity_usd.unwrap_or(0.0);
        let spot_eq = self.live_spot_balance_usd.unwrap_or(0.0);
        let actual_bal = telem_opt.map(|t| t.portfolio_value).unwrap_or_else(|| {
            if live_eq > 0.0 {
                live_eq
            } else {
                spot_eq
            }
        });

        let (risk_pct_desc, risk_usd_val) = match cur.compiled.config.sizing.mode {
            SizingMode::RiskPct => {
                let pct = cur.compiled.config.sizing.risk_pct;
                let usd = if actual_bal > 0.0 {
                    actual_bal * (pct / 100.0)
                } else {
                    0.0
                };
                (format!("{pct:.1}%"), usd)
            }
            SizingMode::FixedUsd => {
                let usd = cur.compiled.config.sizing.fixed_usd;
                (format!("${usd:.2}"), usd)
            }
        };

        let raw_strat_name = cur
            .file_name
            .strip_suffix(".toml")
            .unwrap_or(&cur.file_name);
        let strat_display = raw_strat_name.replace('_', " ");

        let active_runs = live::active_runs();
        let running_coins: Vec<String> = active_runs
            .iter()
            .filter(|h| h.snapshot().running)
            .map(|h| h.coin.clone())
            .collect();
        let armed_list = if !running_coins.is_empty() {
            let mut unique_coins = Vec::new();
            for c in running_coins {
                if !unique_coins.contains(&c) {
                    unique_coins.push(c);
                }
            }
            unique_coins.join(", ")
        } else if cur.selected_markets.is_empty() {
            cur.compiled.config.market.coin.clone()
        } else {
            cur.selected_markets.join(", ")
        };

        let fills_guard = self.live_trades.lock().ok();
        let fills = fills_guard.as_deref().cloned().unwrap_or_default();

        let model_markets: &[String] = if !cur.selected_markets.is_empty() {
            &cur.selected_markets
        } else {
            std::slice::from_ref(&cur.compiled.config.market.coin)
        };

        let coin_matches_model = |coin: &str| -> bool {
            model_markets
                .iter()
                .any(|m| norm_coin(m).eq_ignore_ascii_case(norm_coin(coin)))
        };

        let is_close = |f: &LiveTradeFill| f.dir.starts_with("Close") || f.closed_pnl.abs() > 1e-4;

        // Model-specific session closed trades
        let model_session_closes: Vec<&LiveTradeFill> = fills
            .iter()
            .filter(|f| {
                f.time_ms >= self.session_start_ms && coin_matches_model(&f.coin) && is_close(f)
            })
            .collect();
        let session_wins = model_session_closes
            .iter()
            .filter(|f| f.closed_pnl > 0.0)
            .count();
        let session_losses = model_session_closes
            .iter()
            .filter(|f| f.closed_pnl < 0.0)
            .count();
        let session_model_pnl = norm_pnl(model_session_closes.iter().map(|f| f.closed_pnl).sum());

        // Model-specific all-time on-chain closed trades
        let model_all_closes: Vec<&LiveTradeFill> = fills
            .iter()
            .filter(|f| coin_matches_model(&f.coin) && is_close(f))
            .collect();
        let all_time_wins = model_all_closes
            .iter()
            .filter(|f| f.closed_pnl > 0.0)
            .count();
        let all_time_losses = model_all_closes
            .iter()
            .filter(|f| f.closed_pnl < 0.0)
            .count();
        let all_time_model_pnl = norm_pnl(model_all_closes.iter().map(|f| f.closed_pnl).sum());

        let session_total = session_wins + session_losses;
        let session_win_rate = if session_total > 0 {
            format!(
                "{:.1}%",
                (session_wins as f64 / session_total as f64) * 100.0
            )
        } else {
            "—".to_string()
        };
        let session_wl_col = if session_wins > session_losses {
            COL_GREEN
        } else if session_losses > 0 {
            COL_RED
        } else {
            Color::White
        };
        let session_pnl_col = if session_model_pnl > 0.0 {
            COL_GREEN
        } else if session_model_pnl < 0.0 {
            COL_RED
        } else {
            COL_DIM
        };

        let all_time_total = all_time_wins + all_time_losses;
        let all_time_win_rate = if all_time_total > 0 {
            format!(
                "{:.1}%",
                (all_time_wins as f64 / all_time_total as f64) * 100.0
            )
        } else {
            "—".to_string()
        };
        let all_time_wl_col = if all_time_wins > all_time_losses {
            COL_GREEN
        } else if all_time_losses > 0 {
            COL_RED
        } else {
            Color::White
        };
        let all_time_pnl_col = if all_time_model_pnl > 0.0 {
            COL_GREEN
        } else if all_time_model_pnl < 0.0 {
            COL_RED
        } else {
            COL_DIM
        };

        let live_status_span = if self.live_on {
            Span::styled(
                "● LIVE TRADING (ARMED)",
                Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
            )
        } else if self.armed_dry_run {
            Span::styled(
                "● PAPER TRADING (ARMED)",
                Style::default().fg(COL_YELLOW).add_modifier(Modifier::BOLD),
            )
        } else if active_runs.iter().any(|h| h.snapshot().halted) {
            Span::styled(
                "● HALTED (Check Status Bar / Press Space)",
                Style::default().fg(COL_RED).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                "○ STANDBY (Press Space to Arm / 0 for D12)",
                Style::default().fg(COL_YELLOW),
            )
        };

        let avail_usdc = telem_opt.map(|t| t.available_usdc).unwrap_or(spot_eq);
        let portfolio_val = telem_opt.map(|t| t.portfolio_value).unwrap_or(actual_bal);

        let upnl = telem_opt.map(|t| t.total_unrealized_pnl).unwrap_or(0.0);
        let upnl_col = if upnl >= 0.0 { COL_GREEN } else { COL_RED };
        let upnl_sign = if upnl >= 0.0 { "+" } else { "" };

        let empty_positions = Vec::new();
        let open_positions = telem_opt
            .map(|t| &t.open_positions)
            .unwrap_or(&empty_positions);

        let positions_span = if open_positions.is_empty() {
            Span::styled(
                "● FLAT (0 Open Positions)",
                Style::default().fg(COL_HOT_PINK),
            )
        } else {
            let names: Vec<String> = open_positions
                .iter()
                .map(|p| {
                    let p_coin = p.coin.strip_prefix("xyz:").unwrap_or(&p.coin);
                    let p_pnl = if p.unrealized_pnl >= 0.0 {
                        format!("+${:.2}", p.unrealized_pnl)
                    } else {
                        format!("-${:.2}", p.unrealized_pnl.abs())
                    };
                    format!("{p_coin} ({p_pnl})")
                })
                .collect();
            Span::styled(
                format!(
                    "● {} Active: {}  ·  [ c ] Details",
                    open_positions.len(),
                    names.join(", ")
                ),
                Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
            )
        };

        let net_tag = if self.is_mainnet {
            "Mainnet"
        } else {
            "Testnet"
        };

        let full_agent_str = self
            .agent_address
            .or_else(|| {
                EncryptedAgentKeystore::load_from_file(&self.keystore_path())
                    .ok()
                    .map(|k| k.agent_address)
            })
            .map(|a| format!("{:#x}", a))
            .unwrap_or_else(|| "None".to_string());

        let lines = if is_narrow {
            vec![
                // Row 1: Account (Narrow)
                Line::from(vec![
                    Span::styled(" Acct    : ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("${portfolio_val:.2}"),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" ({upnl_sign}${upnl:.2})"),
                        Style::default().fg(upnl_col).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" · ${avail_usdc:.2} Avail"),
                        Style::default().fg(COL_GREEN),
                    ),
                    Span::styled(format!(" · {net_tag}"), Style::default().fg(COL_DIM)),
                ]),
                // Row 2: Agent Key & Copy Hotkey with Spinner Pulse Animation (Narrow)
                {
                    let mut r2 = vec![Span::styled(" Agent   : ", Style::default().fg(COL_DIM))];
                    if self.copy_pulse_ticks > 0 {
                        r2.extend(address_copy_pulse_spans(&full_agent_str, self.anim_tick));
                        r2.push(Span::styled(
                            " ✓",
                            Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                        ));
                    } else {
                        r2.extend(colored_address_spans(&full_agent_str));
                        r2.push(Span::styled(" [y:copy]", Style::default().fg(COL_KEY)));
                    }
                    Line::from(r2)
                },
                // Row 3: Positions (Narrow)
                Line::from(vec![
                    Span::styled(" Pos     : ", Style::default().fg(COL_DIM)),
                    if open_positions.is_empty() {
                        Span::styled("● FLAT (0 Open)", Style::default().fg(COL_HOT_PINK))
                    } else {
                        let parts: Vec<String> = open_positions
                            .iter()
                            .map(|p| {
                                let p_coin = p.coin.strip_prefix("xyz:").unwrap_or(&p.coin);
                                format!(
                                    "{}: {}${:.2}",
                                    p_coin,
                                    if p.unrealized_pnl >= 0.0 { "+" } else { "" },
                                    p.unrealized_pnl
                                )
                            })
                            .collect();
                        Span::styled(
                            format!("● {} Active: {}", open_positions.len(), parts.join(", ")),
                            Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                        )
                    },
                ]),
                // Row 4: Win / Loss (Narrow)
                Line::from(vec![
                    Span::styled(" Win/Loss: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("Sess {session_wins}W/{session_losses}L ({session_win_rate})"),
                        Style::default()
                            .fg(session_wl_col)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" (${session_model_pnl:+.2})"),
                        Style::default().fg(session_pnl_col),
                    ),
                    Span::styled(" │ Acct: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("{all_time_wins}W/{all_time_losses}L ({all_time_win_rate})"),
                        Style::default().fg(all_time_wl_col),
                    ),
                ]),
                // Row 5: Bot State (Narrow)
                Line::from(vec![
                    Span::styled(" Bot     : ", Style::default().fg(COL_DIM)),
                    live_status_span.clone(),
                ]),
                // Row 6: Strategy (Narrow)
                Line::from(vec![
                    Span::styled(" Strat   : ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!(
                            "{} · {} [{}] {}x {}",
                            strat_display,
                            cur.compiled.config.market.coin,
                            cur.compiled.config.market.interval,
                            cur.compiled.config.market.leverage,
                            if cur.compiled.config.market.is_cross {
                                "Cross"
                            } else {
                                "Iso"
                            }
                        ),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                // Row 7: Signal (Narrow)
                Line::from(vec![
                    Span::styled(" Signal  : ", Style::default().fg(COL_DIM)),
                    match &cur.latest_tick_plan {
                        Some(p) => Span::styled(
                            format!(
                                "● TRIGGER: {} {} @ {}",
                                p.side.as_str(),
                                p.coin,
                                p.entry_px_str
                            ),
                            Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                        ),
                        None => Span::styled(
                            format!("● FLAT on {}", cur.compiled.config.market.coin),
                            Style::default().fg(COL_HOT_PINK),
                        ),
                    },
                ]),
            ]
        } else {
            vec![
                // Row 1: Clean Trader Account Summary (Wide)
                Line::from(vec![
                    Span::styled(" Account  : ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("${portfolio_val:.2} Equity"),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ·  ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("{upnl_sign}${upnl:.2} uPnL"),
                        Style::default().fg(upnl_col).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ·  ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("${avail_usdc:.2} Avail"),
                        Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        if let Some(t) = telem_opt {
                            if t.spot_hold > 0.0 {
                                format!(" (${:.2} margin)", t.spot_hold)
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        },
                        Style::default().fg(COL_YELLOW),
                    ),
                    Span::styled("  ·  Press ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        "[ c ]",
                        Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" for Web dApp", Style::default().fg(Color::White)),
                ]),
                // Row 2: Hyperliquid Agent Address (Full Address & Copy Hotkey with Spinner Pulse Animation)
                {
                    let mut row2_spans =
                        vec![Span::styled(" Agent Key: ", Style::default().fg(COL_DIM))];
                    if self.copy_pulse_ticks > 0 {
                        row2_spans
                            .extend(address_copy_pulse_spans(&full_agent_str, self.anim_tick));
                        row2_spans.push(Span::styled("  │  ", Style::default().fg(COL_DIM)));
                        row2_spans.push(Span::styled(
                            "✓ Copied to Clipboard!",
                            Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                        ));
                    } else {
                        row2_spans.extend(colored_address_spans(&full_agent_str));
                        row2_spans.push(Span::styled("  │  Press ", Style::default().fg(COL_DIM)));
                        row2_spans.push(Span::styled(
                            "[ y ]",
                            Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                        ));
                        row2_spans.push(Span::styled(" to Copy", Style::default().fg(COL_GREEN)));
                    }
                    Line::from(row2_spans)
                },
                // Row 3: Live Positions (Wide)
                Line::from(vec![
                    Span::styled(" Positions: ", Style::default().fg(COL_DIM)),
                    positions_span,
                ]),
                // Row 4: Win / Loss Record (Wide)
                Line::from(vec![
                    Span::styled(" Win/Loss : ", Style::default().fg(COL_DIM)),
                    Span::styled("Session: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("{session_wins}W / {session_losses}L ({session_win_rate})"),
                        Style::default()
                            .fg(session_wl_col)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" (${session_model_pnl:+.2} USD)"),
                        Style::default().fg(session_pnl_col),
                    ),
                    Span::styled("  │  Account Fills: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("{all_time_wins}W / {all_time_losses}L ({all_time_win_rate})"),
                        Style::default()
                            .fg(all_time_wl_col)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" (${all_time_model_pnl:+.2} USD)"),
                        Style::default().fg(all_time_pnl_col),
                    ),
                ]),
                // Row 5: Live Bot Execution State & Armed Markets (Wide)
                Line::from(vec![
                    Span::styled(" Bot State: ", Style::default().fg(COL_DIM)),
                    live_status_span,
                    Span::styled("  │  Armed Markets: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        armed_list,
                        Style::default()
                            .fg(COL_NEON_BLUE)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                // Row 6: Strategy, Market, Leverage & Risk (Wide)
                Line::from(vec![
                    Span::styled(" Strategy : ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        strat_display,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ·  Market: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!(
                            "{} [{}]",
                            cur.compiled.config.market.coin, cur.compiled.config.market.interval
                        ),
                        Style::default()
                            .fg(COL_NEON_BLUE)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ·  Lev: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!(
                            "{}x {}",
                            cur.compiled.config.market.leverage,
                            if cur.compiled.config.market.is_cross {
                                "Cross"
                            } else {
                                "Iso"
                            }
                        ),
                        Style::default().fg(COL_YELLOW).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ·  Risk: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        if actual_bal > 0.0
                            || matches!(cur.compiled.config.sizing.mode, SizingMode::FixedUsd)
                        {
                            format!("1R = ${risk_usd_val:.2} ({risk_pct_desc})")
                        } else {
                            format!("1R = — ({risk_pct_desc})")
                        },
                        Style::default().fg(Color::White),
                    ),
                ]),
                // Row 7: Real-Time Strategy Signal (Wide)
                Line::from(vec![
                    Span::styled(" Signal   : ", Style::default().fg(COL_DIM)),
                    match &cur.latest_tick_plan {
                        Some(p) => Span::styled(
                            format!(
                                "● TRIGGER ACTIVE: {} {} @ {} | SL={} | TP={} (${:.2} Notional)",
                                p.side.as_str(),
                                p.coin,
                                p.entry_px_str,
                                p.stop_px_str,
                                p.tp_px_str,
                                p.notional_usd
                            ),
                            Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                        ),
                        None => Span::styled(
                            format!(
                                "● FLAT on {} (Waiting for high-conviction trigger candle)",
                                cur.compiled.config.market.coin
                            ),
                            Style::default().fg(COL_HOT_PINK),
                        ),
                    },
                ]),
            ]
        };

        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    pub(super) fn draw_live_trade_tape(&self, frame: &mut Frame, area: Rect) {
        let fills_guard = self.live_trades.lock().ok();
        let fills = fills_guard.as_deref().cloned().unwrap_or_default();

        let today_uk = uk_day_number(now_unix_ms());
        let today_fills: Vec<&LiveTradeFill> = fills
            .iter()
            .filter(|f| uk_day_number(f.time_ms) == today_uk)
            .collect();
        let today_pnl = norm_pnl(today_fills.iter().map(|f| f.closed_pnl).sum());
        let today_count = today_fills.len();

        let session_fills: Vec<&LiveTradeFill> = fills
            .iter()
            .filter(|f| f.time_ms >= self.session_start_ms)
            .collect();
        let session_pnl = norm_pnl(session_fills.iter().map(|f| f.closed_pnl).sum());
        let session_count = session_fills.len();

        let displayed_fills: &Vec<&LiveTradeFill> = if self.show_all_day_trades {
            &today_fills
        } else {
            &session_fills
        };
        let disp_pnl = norm_pnl(displayed_fills.iter().map(|f| f.closed_pnl).sum());
        let total = displayed_fills.len();

        let title = if self.show_all_day_trades {
            format!(
                " ● LIVE Trade Tape — Today's On-Chain History [UK Day] ({} fills · ${:+.2} Realized P&L) · [v] View Run ({} fills · ${:+.2}) · Backing: {:#x} ",
                total,
                disp_pnl,
                session_count,
                session_pnl,
                self.backing_account.unwrap_or(Address::ZERO)
            )
        } else {
            format!(
                " ● LIVE Trade Tape — Current Run ({} fills · ${:+.2} Realized P&L) · [v] Today's History [UK Day] ({} fills · ${:+.2}) · [r] Reset · Backing: {:#x} ",
                total,
                disp_pnl,
                today_count,
                today_pnl,
                self.backing_account.unwrap_or(Address::ZERO)
            )
        };
        let block = Block::default()
            .title(Span::styled(
                title,
                Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COL_GREEN));

        if displayed_fills.is_empty() {
            let msg = if !self.show_all_day_trades && today_count > 0 {
                vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::styled(
                            "  ● CURRENT SESSION CLEAN — ",
                            Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("0 fills executed during this bot run ($0.00 Realized P&L)."),
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("  Exchange History : ", Style::default().fg(COL_DIM)),
                        Span::styled(
                            format!("{today_count} prior on-chain fills from today on Hyperliquid (${today_pnl:+.2} USD)."),
                            Style::default().fg(COL_YELLOW),
                        ),
                        Span::styled(" Press [v] to view today's full history.", Style::default().fg(COL_KEY)),
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("  Session Status   : ", Style::default().fg(COL_DIM)),
                        if self.armed_dry_run || self.live_on {
                            Span::styled(
                                "Auto-Trader ARMED & evaluating live closed bars. Live fills appear immediately.",
                                Style::default().fg(Color::White),
                            )
                        } else {
                            Span::styled(
                                "Standing by / PAUSED. Press [Space] to Arm live strategy execution.",
                                Style::default().fg(COL_YELLOW),
                            )
                        },
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("  Backing Address  : ", Style::default().fg(COL_DIM)),
                        Span::styled(
                            format!("{:#x}", self.backing_account.unwrap_or(Address::ZERO)),
                            Style::default().fg(COL_NEON_BLUE),
                        ),
                        Span::styled(" (Arbitrum One Settled)", Style::default().fg(COL_DIM)),
                    ]),
                ]
            } else {
                vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::styled(
                            "  ● LIVE SESSION ACTIVE — ",
                            Style::default()
                                .fg(COL_GREEN)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(
                            "Only real on-chain Hyperliquid fills from the current live session appear here.",
                        ),
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("  Session Status : ", Style::default().fg(COL_DIM)),
                        if self.armed_dry_run || self.live_on {
                            Span::styled(
                                "Auto-Trader ARMED & evaluating live closed bars. Live fills appear immediately.",
                                Style::default().fg(Color::White),
                            )
                        } else {
                            Span::styled(
                                "Standing by / PAUSED. Press [Space] to Arm live strategy execution.",
                                Style::default().fg(COL_YELLOW),
                            )
                        },
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("  Backing Address: ", Style::default().fg(COL_DIM)),
                        Span::styled(
                            format!("{:#x}", self.backing_account.unwrap_or(Address::ZERO)),
                            Style::default().fg(COL_NEON_BLUE),
                        ),
                        Span::styled(" (Arbitrum One Settled)", Style::default().fg(COL_DIM)),
                    ]),
                ]
            };
            frame.render_widget(Paragraph::new(msg).block(block), area);
            return;
        }

        let is_narrow = area.width < 85;
        let rows: Vec<Row> = displayed_fills
            .iter()
            .rev()
            .skip(self.trade_scroll)
            .take(15)
            .map(|t| {
                let c = if t.closed_pnl > 0.0 {
                    COL_GREEN
                } else if t.closed_pnl == 0.0 {
                    Color::White
                } else {
                    COL_RED
                };
                let time_str = {
                    let secs = (t.time_ms / 1000) % 86400;
                    format!(
                        "{:02}:{:02}:{:02}",
                        secs / 3600,
                        (secs % 3600) / 60,
                        secs % 60
                    )
                };
                if is_narrow {
                    Row::new(vec![
                        time_str,
                        t.coin.clone(),
                        t.dir.clone(),
                        format!("{:.4}", t.px),
                        format!("{:.4}", t.sz),
                        format!("${:+.2}", norm_pnl(t.closed_pnl)),
                    ])
                    .style(Style::default().fg(c))
                } else {
                    Row::new(vec![
                        format!("#{}", t.oid),
                        time_str,
                        t.coin.clone(),
                        t.dir.clone(),
                        format!("{:.4}", t.px),
                        format!("{:.4}", t.sz),
                        format!("${:.4}", t.fee),
                        format!("${:+.2}", norm_pnl(t.closed_pnl)),
                    ])
                    .style(Style::default().fg(c))
                }
            })
            .collect();

        let (header, widths) = if is_narrow {
            (
                Row::new(vec![
                    "TIME (UTC)",
                    "COIN",
                    "DIRECTION",
                    "FILL PX",
                    "SIZE",
                    "REALIZED PNL",
                ])
                .style(Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD)),
                vec![
                    Constraint::Length(10),
                    Constraint::Length(8),
                    Constraint::Length(10),
                    Constraint::Length(10),
                    Constraint::Length(8),
                    Constraint::Min(12),
                ],
            )
        } else {
            (
                Row::new(vec![
                    "ORDER ID",
                    "TIME (UTC)",
                    "COIN",
                    "DIRECTION",
                    "FILL PX",
                    "SIZE",
                    "FEE",
                    "REALIZED PNL",
                ])
                .style(Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD)),
                vec![
                    Constraint::Length(10),
                    Constraint::Length(12),
                    Constraint::Length(8),
                    Constraint::Length(14),
                    Constraint::Length(12),
                    Constraint::Length(10),
                    Constraint::Length(10),
                    Constraint::Min(14),
                ],
            )
        };

        let table = Table::new(rows, widths).header(header).block(block);
        frame.render_widget(table, area);
    }

    pub(super) fn draw_quick_connect(&self, frame: &mut Frame, area: Rect) {
        let popup_width = 78.min(area.width.saturating_sub(4));
        let popup_height = 20.min(area.height.saturating_sub(2));
        let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
        let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let backing_str = self
            .backing_account
            .map(|a| format!("{a:#x}"))
            .unwrap_or_else(|| "0x... (Active Vaughan Wallet Account)".to_string());

        let has_keystore = self.keystore_path().exists();

        let header_str = if self.is_mainnet {
            if has_keystore {
                "   HYPERLIQUID MAINNET — UNLOCK AGENT KEY"
            } else {
                "   HYPERLIQUID MAINNET QUICK-CONNECT"
            }
        } else {
            if has_keystore {
                "   HYPERLIQUID TESTNET — UNLOCK AGENT KEY"
            } else {
                "   HYPERLIQUID TESTNET QUICK-CONNECT"
            }
        };

        let action_desc = if has_keystore {
            " to Enter Password & Unlock Agent Key."
        } else if self.is_mainnet {
            " to 1-Click Authorize Agent Key on Mainnet."
        } else {
            " to 1-Click Connect & Authorize Agent Key."
        };

        let mut lines = vec![
            Line::from(""),
            Line::from(vec![Span::styled(
                header_str,
                Style::default()
                    .fg(COL_NEON_BLUE)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from(vec![
                Span::styled("   • Backing Account : ", Style::default().fg(COL_DIM)),
                Span::styled(
                    if self.is_mainnet {
                        format!("{backing_str} (Arbitrum One Settled)")
                    } else {
                        backing_str
                    },
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ];

        if has_keystore {
            if let Some(agent) = EncryptedAgentKeystore::load_from_file(&self.keystore_path())
                .ok()
                .map(|k| k.agent_address)
            {
                let agent_str = format!("{agent:#x}");
                let mut agent_line_spans = vec![Span::styled(
                    "   • Agent Address   : ",
                    Style::default().fg(COL_DIM),
                )];
                if self.copy_pulse_ticks > 0 {
                    agent_line_spans.extend(address_copy_pulse_spans(&agent_str, self.anim_tick));
                    agent_line_spans.push(Span::styled(
                        "  │  ✓ Copied!",
                        Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                    ));
                } else {
                    agent_line_spans.extend(colored_address_spans(&agent_str));
                    agent_line_spans
                        .push(Span::styled("  │  Press ", Style::default().fg(COL_DIM)));
                    agent_line_spans.push(Span::styled(
                        "[ y ]",
                        Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                    ));
                    agent_line_spans.push(Span::styled(" to Copy", Style::default().fg(COL_GREEN)));
                }
                lines.push(Line::from(agent_line_spans));
            }
        }

        lines.extend(vec![
            Line::from(vec![
                Span::styled("   • Target Network  : ", Style::default().fg(COL_DIM)),
                Span::styled(
                    if self.is_mainnet {
                        "Hyperliquid Mainnet (api.hyperliquid.xyz)"
                    } else {
                        "Hyperliquid Testnet (api.hyperliquid-testnet.xyz)"
                    },
                    if self.is_mainnet {
                        Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    },
                ),
            ]),
            Line::from(vec![
                Span::styled("   • Security Role   : ", Style::default().fg(COL_DIM)),
                Span::styled(
                    "Isolated trading key (0600) — cannot withdraw funds",
                    Style::default().fg(COL_GREEN),
                ),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("   Press ", Style::default().fg(COL_DIM)),
                Span::styled(
                    "[ Enter ]",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" or ", Style::default().fg(COL_DIM)),
                Span::styled(
                    "[ a ]",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" or ", Style::default().fg(COL_DIM)),
                Span::styled(
                    "[ u ]",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    action_desc,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("   Press ", Style::default().fg(COL_DIM)),
                Span::styled(
                    "[   n   ]",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " to Toggle Network Mode (Mainnet <-> Testnet).",
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("   Press ", Style::default().fg(COL_DIM)),
                Span::styled(
                    "[   w   ]",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if self.is_mainnet {
                        " to open Hyperliquid (app.hyperliquid.xyz/trade) in VB."
                    } else {
                        " to open Testnet Faucet in VB and claim free 1,000 USDC."
                    },
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("   Press ", Style::default().fg(COL_DIM)),
                Span::styled(
                    "[  Esc  ]",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" to return to Dashboard.", Style::default().fg(COL_DIM)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                format!("   {}", self.status_msg),
                Style::default().fg(COL_YELLOW),
            )),
        ]);

        let title = if has_keystore {
            " Unlock Agent Key "
        } else {
            " 1-Click Setup "
        };

        let block = Block::default()
            .title(Span::styled(
                title,
                Style::default()
                    .fg(COL_HOT_PINK)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COL_PURPLE));

        frame.render_widget(Paragraph::new(lines).block(block), popup_area);
    }

    pub(super) fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let line = if !self.is_connected {
            let has_ks = self.keystore_path().exists();
            Line::from(vec![
                Span::styled(
                    if has_ks {
                        " Enter / a / u"
                    } else {
                        " Enter / a"
                    },
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(if has_ks {
                    " Unlock Keystore  "
                } else {
                    " 1-Click Connect  "
                }),
                Span::styled(
                    "c",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Hyperliquid  "),
                Span::styled(
                    "y",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Copy  "),
                Span::styled(
                    "v",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Hist/Run  "),
                Span::styled(
                    "r",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Reset Run  "),
                Span::styled(
                    "n",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Toggle Net  "),
                Span::styled(
                    "w",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(if self.is_mainnet {
                    " Open HL  "
                } else {
                    " Faucet (/drip)  "
                }),
                Span::styled(
                    "Esc",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Return   │ "),
                Span::styled(&self.status_msg, Style::default().fg(COL_YELLOW)),
            ])
        } else if self.focus == HlFocus::Markets {
            Line::from(vec![
                Span::styled(
                    " Tab/←",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Strategies  "),
                Span::styled(
                    "↑/↓",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Market  "),
                Span::styled(
                    "x",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Toggle [x]  "),
                Span::styled(
                    "a",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" All/None  "),
                Span::styled(
                    "+",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Add Ticker  "),
                Span::styled(
                    "Space/Enter",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Start/Stop LIVE  "),
                Span::styled(
                    "Esc",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Back │ "),
                Span::styled(&self.status_msg, Style::default().fg(COL_YELLOW)),
            ])
        } else {
            Line::from(vec![
                Span::styled(
                    " ↑/↓",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Strategy  "),
                Span::styled(
                    "Tab/m",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Markets Col  "),
                Span::styled(
                    "Space",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Start/Stop LIVE  "),
                Span::styled(
                    "0",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Arm D12 Combo  "),
                Span::styled(
                    "+",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Add Market  "),
                Span::styled(
                    "[/]",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Tape  "),
                Span::styled(
                    "c",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Hyperliquid  "),
                Span::styled(
                    "y",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Copy  "),
                Span::styled(
                    "v",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Hist/Run  "),
                Span::styled(
                    "r",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Reset Run  "),
                Span::styled(
                    "w",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(if self.is_mainnet {
                    " Hyperliquid  "
                } else {
                    " Testnet/Faucet  "
                }),
                Span::styled(
                    "x",
                    Style::default().fg(COL_RED).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Kill  "),
                Span::styled(
                    "Esc",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Return │ "),
                Span::styled(&self.status_msg, Style::default().fg(COL_YELLOW)),
            ])
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COL_BORDER));
        frame.render_widget(Paragraph::new(line).block(block), area);
    }
}
