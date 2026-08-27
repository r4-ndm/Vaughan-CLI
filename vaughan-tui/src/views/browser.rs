//! Contract Browser & DEX REPL view (`wiz4rd-engine`).
//!
//! Stateful interactive terminal REPL for inspecting contracts, probing
//! interfaces (ERC-20, Uniswap V2/V3, Multicall), executing read-only dynamic
//! calls (`alloy-dyn-abi`), gated writes (`write` / `writeraw` → fee confirm →
//! broadcast), discovering pairs/pools, and intent macros (`/swap`, `/inspect`, …).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_core::browser::abi::AbiResolution;
use vaughan_core::browser::events::PairDiscovery;
use vaughan_core::browser::probe::ContractFingerprint;
use vaughan_core::browser::selectors::selector_to_hex;
use vaughan_core::browser::{BrowserEngine, ContractInspection};
use vaughan_core::chains::{EvmTransaction, Fee};
use vaughan_core::core::{TransactionService, WalletState};
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::intent::parse_intent;
use crate::jobs::{spinner_frame, UiJob, UiJobResult};
use crate::views::{body_areas, status_paragraph};
use alloy::primitives::Address;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Repl,
    ConfirmWrite,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Idle,
    EstimatingFee,
    Sending,
}

/// Stateful Browser REPL View.
pub struct BrowserView {
    engine: BrowserEngine,
    active_address: Option<Address>,
    current_inspection: Option<ContractInspection>,
    input: Input,
    history: Vec<String>,
    history_index: Option<usize>,
    logs: Vec<Line<'static>>,
    scroll_offset: usize,
    status: String,
    stage: Stage,
    busy: Busy,
    tick: u64,
    pending_tx: Option<EvmTransaction>,
    pending_fee: Option<Fee>,
    confirm_lines: Vec<String>,
}

impl Default for BrowserView {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserView {
    pub fn new() -> Self {
        let logs = vec![
            Line::from(vec![
                Span::styled(
                    "⚡ Vaughan Contract Browser & DEX Engine ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("(`wiz4rd-engine`)", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(vec![
                Span::raw("Type "),
                Span::styled("browse <0xAddress>", Style::default().fg(Color::Yellow)),
                Span::raw(" to inspect, or "),
                Span::styled("help", Style::default().fg(Color::Green)),
                Span::raw(" for available commands."),
            ]),
            Line::from(""),
        ];

        Self {
            engine: BrowserEngine::new(),
            active_address: None,
            current_inspection: None,
            input: Input::new(
                false,
                "Enter command (e.g. browse 0x..., call name(), pairs, help)...",
            ),
            history: Vec::new(),
            history_index: None,
            logs,
            scroll_offset: 0,
            status: String::new(),
            stage: Stage::Repl,
            busy: Busy::Idle,
            tick: 0,
            pending_tx: None,
            pending_fee: None,
            confirm_lines: Vec::new(),
        }
    }

    /// Animation tick for fee/send spinners.
    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    /// Inspect an address (used by `/inspect` intent from other screens).
    pub fn browse_address(&mut self, addr_str: &str, wallet: &WalletState, handle: &Handle) {
        self.cmd_browse(addr_str, wallet, handle);
    }

    pub fn apply_job_result(&mut self, result: UiJobResult) {
        match result {
            UiJobResult::Fee(Ok(fee)) => {
                self.pending_fee = Some(fee.clone());
                self.busy = Busy::Idle;
                self.status = format!(
                    "Est. fee ~{} {} · Enter broadcast · Esc cancel",
                    fee.total, fee.currency
                );
            }
            UiJobResult::Fee(Err(e)) => {
                self.busy = Busy::Idle;
                self.stage = Stage::Repl;
                self.pending_tx = None;
                self.status = e.user_message();
                self.log_error(&format!("Fee estimate failed: {}", e.user_message()));
            }
            UiJobResult::Send(Ok(receipt)) => {
                let hash = receipt.hash;
                self.busy = Busy::Idle;
                self.stage = Stage::Repl;
                self.pending_tx = None;
                self.pending_fee = None;
                self.confirm_lines.clear();
                self.status = format!("Broadcast {hash}");
                self.logs.push(Line::from(vec![
                    Span::styled(
                        "✔ Write tx: ",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(hash),
                ]));
                self.logs.push(Line::from(""));
            }
            UiJobResult::Send(Err(e)) => {
                self.busy = Busy::Idle;
                self.stage = Stage::Repl;
                self.pending_tx = None;
                self.pending_fee = None;
                self.status = e.user_message();
                self.log_error(&format!("Write failed: {}", e.user_message()));
            }
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let net = wallet.networks().active();

        if self.stage == Stage::ConfirmWrite {
            let title = brand::fade_line(" Confirm contract write ");
            let inner = brand::render_faded_box(frame, content, Some(title));
            let mut lines: Vec<Line> = self
                .confirm_lines
                .iter()
                .map(|s| Line::from(s.clone()))
                .collect();
            lines.push(Line::from(""));
            match self.busy {
                Busy::EstimatingFee => {
                    lines.push(Line::from(format!(
                        "{} estimating fee…",
                        spinner_frame(self.tick)
                    )));
                }
                Busy::Sending => {
                    lines.push(Line::from(format!(
                        "{} broadcasting…",
                        spinner_frame(self.tick)
                    )));
                }
                Busy::Idle => {
                    if let Some(fee) = &self.pending_fee {
                        lines.push(Line::from(format!(
                            "Est. fee: {} {}",
                            fee.total, fee.currency
                        )));
                    }
                    lines.push(Line::from("Enter approve · Esc cancel"));
                }
            }
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
            frame.render_widget(status_paragraph(&self.status), status_area);
            return;
        }

        // Split content into header (context info), log pane (scrollable output), and prompt
        let chunks = Layout::vertical([
            Constraint::Length(3), // Context / active contract bar
            Constraint::Min(6),    // Output logs pane
            Constraint::Length(3), // Input prompt line
        ])
        .split(content);

        // 1. Context Header
        let active_str = match self.active_address {
            Some(addr) => addr.to_checksum(None),
            None => "(None - type `browse 0x...`)".to_string(),
        };

        let fingerprint_str = match &self.current_inspection {
            Some(insp) => match &insp.fingerprint {
                ContractFingerprint::Erc20 {
                    name,
                    symbol,
                    decimals,
                } => {
                    format!(
                        " [ERC-20: {} ({}) | Dec: {}]",
                        name.as_deref().unwrap_or("?"),
                        symbol.as_deref().unwrap_or("?"),
                        decimals
                            .map(|d| d.to_string())
                            .unwrap_or_else(|| "?".to_string())
                    )
                }
                ContractFingerprint::UniswapV2Pair {
                    reserve0, reserve1, ..
                } => {
                    let price_str = if let Some(p) = insp.fingerprint.v2_spot_price(18, 18) {
                        format!(" | Spot: {:.6}", p)
                    } else {
                        String::new()
                    };
                    format!(
                        " [V2 Pair | Reserves: {}, {}{}]",
                        reserve0, reserve1, price_str
                    )
                }
                ContractFingerprint::UniswapV2Factory { all_pairs_length } => {
                    format!(" [V2 Factory | Pairs: {}]", all_pairs_length.unwrap_or(0))
                }
                ContractFingerprint::UniswapV3Pool {
                    fee,
                    tick,
                    liquidity,
                    ..
                } => {
                    let price_str = if let Some(p) = insp.fingerprint.v3_spot_price(18, 18) {
                        format!(" | Price: {:.6}", p)
                    } else {
                        String::new()
                    };
                    let tick_str = tick.map(|t| format!(" | Tick: {}", t)).unwrap_or_default();
                    let liq_str = liquidity
                        .map(|l| format!(" | Liq: {}", l))
                        .unwrap_or_default();
                    format!(
                        " [V3 Pool | Fee: {}{}{}{}]",
                        fee, price_str, tick_str, liq_str
                    )
                }
                ContractFingerprint::UniswapV3Factory => " [V3 Factory]".to_string(),
                ContractFingerprint::Multicall3 => " [Multicall3]".to_string(),
                ContractFingerprint::Weth => " [Wrapped Native]".to_string(),
                ContractFingerprint::Generic { has_code, .. } => {
                    if *has_code {
                        " [Generic Contract]".to_string()
                    } else {
                        " [EOA / No Bytecode]".to_string()
                    }
                }
            },
            None => String::new(),
        };

        let header_text = vec![Line::from(vec![
            Span::raw("Network: "),
            Span::styled(
                format!("{} (Chain ID {})", net.name, net.chain_id),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  | Target: "),
            Span::styled(
                active_str,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(fingerprint_str, Style::default().fg(Color::Green)),
        ])];
        let ctx_inner =
            brand::render_faded_box(frame, chunks[0], Some(brand::fade_line(" Context ")));
        frame.render_widget(Paragraph::new(header_text), ctx_inner);

        // 2. Output Logs Pane
        let visible_height = chunks[1].height.saturating_sub(2) as usize;
        let total_logs = self.logs.len();
        let skip = if total_logs > visible_height {
            total_logs.saturating_sub(visible_height + self.scroll_offset)
        } else {
            0
        };

        let slice = self.logs.iter().skip(skip).cloned().collect::<Vec<_>>();
        let logs_inner = brand::render_faded_box(
            frame,
            chunks[1],
            Some(brand::fade_line(" REPL Console (PgUp/PgDn to scroll) ")),
        );
        frame.render_widget(Paragraph::new(slice).wrap(Wrap { trim: false }), logs_inner);

        // 3. Input Prompt Line
        let prompt_line = self.input.line();
        let cmd_inner = brand::render_faded_box(
            frame,
            chunks[2],
            Some(brand::fade_line(" Command (Esc to exit) ")),
        );
        frame.render_widget(Paragraph::new(prompt_line), cmd_inner);

        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        if self.busy != Busy::Idle {
            return KeyOutcome::Consumed;
        }

        if self.stage == Stage::ConfirmWrite {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.stage = Stage::Repl;
                    self.pending_tx = None;
                    self.pending_fee = None;
                    self.confirm_lines.clear();
                    self.status = "Write cancelled".into();
                    KeyOutcome::Consumed
                }
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let Some(fee) = self.pending_fee.clone() else {
                        self.status = "fee estimate missing — Esc back and retry".into();
                        return KeyOutcome::Consumed;
                    };
                    let Some(tx) = self.pending_tx.take() else {
                        self.status = "transaction payload missing — Esc back and retry".into();
                        return KeyOutcome::Consumed;
                    };
                    self.pending_fee = None;
                    self.busy = Busy::Sending;
                    KeyOutcome::StartJob(UiJob::SendEvmWithFee { tx, fee })
                }
                _ => KeyOutcome::Consumed,
            };
        }

        match key.code {
            KeyCode::Esc => {
                if !self.input.value().is_empty() {
                    self.input.take_string();
                    KeyOutcome::Consumed
                } else {
                    KeyOutcome::Navigate(Screen::Dashboard)
                }
            }
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(5);
                KeyOutcome::Consumed
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(5);
                KeyOutcome::Consumed
            }
            KeyCode::Up => {
                if !self.history.is_empty() {
                    let next_idx = match self.history_index {
                        Some(idx) => idx.saturating_sub(1),
                        None => self.history.len().saturating_sub(1),
                    };
                    self.history_index = Some(next_idx);
                    if let Some(cmd) = self.history.get(next_idx) {
                        self.input.set_value(cmd.clone());
                    }
                }
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                if let Some(idx) = self.history_index {
                    if idx + 1 < self.history.len() {
                        let next_idx = idx + 1;
                        self.history_index = Some(next_idx);
                        if let Some(cmd) = self.history.get(next_idx) {
                            self.input.set_value(cmd.clone());
                        }
                    } else {
                        self.history_index = None;
                        self.input.take_string();
                    }
                }
                KeyOutcome::Consumed
            }
            _ => {
                let action = self.input.handle_key(key);
                match action {
                    InputAction::Submitted => {
                        let cmd = self.input.take_string().trim().to_string();
                        if cmd.is_empty() {
                            return KeyOutcome::Consumed;
                        }
                        self.history.push(cmd.clone());
                        self.history_index = None;
                        self.execute_command(&cmd, wallet, handle)
                    }
                    InputAction::Consumed => KeyOutcome::Consumed,
                    InputAction::Ignored => KeyOutcome::NotHandled,
                }
            }
        }
    }

    fn execute_command(&mut self, cmd: &str, wallet: &WalletState, handle: &Handle) -> KeyOutcome {
        self.scroll_offset = 0;
        self.logs.push(Line::from(vec![
            Span::styled("❯ ", Style::default().fg(Color::Yellow)),
            Span::raw(cmd.to_string()),
        ]));

        if let Some(parsed) = parse_intent(cmd) {
            match parsed {
                Ok(nav) => {
                    self.logs.push(Line::from(format!("→ intent {nav:?}")));
                    self.logs.push(Line::from(""));
                    // Same-screen inspect: browse without leaving Browser.
                    if let crate::intent::IntentNav::BrowserInspect { address } = &nav {
                        self.cmd_browse(address, wallet, handle);
                        self.logs.push(Line::from(""));
                        return KeyOutcome::Consumed;
                    }
                    return KeyOutcome::Intent(nav);
                }
                Err(e) => {
                    self.log_error(&e);
                    self.logs.push(Line::from(""));
                    return KeyOutcome::Consumed;
                }
            }
        }

        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            return KeyOutcome::Consumed;
        }

        let outcome = match parts[0].to_lowercase().as_str() {
            "help" | "?" => {
                self.cmd_help();
                KeyOutcome::Consumed
            }
            "clear" | "cls" => {
                self.logs.clear();
                KeyOutcome::Consumed
            }
            "browse" | "b" | "inspect" => {
                if parts.len() < 2 {
                    self.log_error("Usage: browse <0xContractAddress>");
                    KeyOutcome::Consumed
                } else {
                    self.cmd_browse(parts[1], wallet, handle);
                    KeyOutcome::Consumed
                }
            }
            "probe" | "p" => {
                self.cmd_probe(wallet, handle);
                KeyOutcome::Consumed
            }
            "info" | "i" => {
                self.cmd_info();
                KeyOutcome::Consumed
            }
            "call" => {
                if parts.len() < 2 {
                    self.log_error("Usage: call <function_name> [arg1 arg2 ...]");
                } else {
                    let func_name = parts[1];
                    let args: Vec<String> = parts[2..].iter().map(|s| s.to_string()).collect();
                    self.cmd_call(func_name, &args, wallet, handle);
                }
                KeyOutcome::Consumed
            }
            "callraw" => {
                if parts.len() < 2 {
                    self.log_error("Usage: callraw <0xHexCalldata>");
                } else {
                    self.cmd_call_raw(parts[1], wallet, handle);
                }
                KeyOutcome::Consumed
            }
            "write" | "send" => self.cmd_write(&parts[1..], wallet),
            "writeraw" | "sendraw" => {
                if parts.len() < 2 {
                    self.log_error("Usage: writeraw <0xHexCalldata> [value <wei>]");
                    KeyOutcome::Consumed
                } else {
                    self.cmd_write_raw(&parts[1..], wallet)
                }
            }
            "pairs" => {
                let limit = parts
                    .get(1)
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(10);
                self.cmd_pairs(0, limit, wallet, handle);
                KeyOutcome::Consumed
            }
            "sign-typed" => {
                let Some(raw) = parts.get(1) else {
                    self.log_error("Usage: sign-typed <json> | sign-typed @/path/to/file.json");
                    return KeyOutcome::Consumed;
                };
                match load_typed_data_for_repl(raw) {
                    Ok(v) => KeyOutcome::SignTypedData(v),
                    Err(e) => {
                        self.log_error(&e);
                        KeyOutcome::Consumed
                    }
                }
            }
            other => {
                self.log_error(&format!(
                    "Unknown command '{}'. Type 'help' for commands.",
                    other
                ));
                KeyOutcome::Consumed
            }
        };
        self.logs.push(Line::from(""));
        outcome
    }

    fn cmd_help(&mut self) {
        self.logs.push(Line::from(vec![Span::styled(
            "Available Commands:",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]));
        self.logs.push(Line::from(
            "  browse <0xAddress>         Inspect contract (resolves ABI & probes capability)",
        ));
        self.logs.push(Line::from(
            "  probe                      Re-run capability fingerprinting on current contract",
        ));
        self.logs.push(Line::from(
            "  info                       Show contract code size, chain ID, and ABI status",
        ));
        self.logs.push(Line::from(
            "  call <fn_name> [args...]   Read-only eth_call against verified ABI",
        ));
        self.logs.push(Line::from(
            "  callraw <0xCalldata>       Read-only eth_call with raw calldata",
        ));
        self.logs.push(Line::from(
            "  write <fn> [args...] [value <wei>]   Encode write → fee confirm → broadcast",
        ));
        self.logs.push(Line::from(
            "  writeraw <0xCalldata> [value <wei>]  Raw write → fee confirm → broadcast",
        ));
        self.logs.push(Line::from(
            "  pairs [limit]              List pairs if current contract is a V2 factory",
        ));
        self.logs.push(Line::from(
            "  /swap [amt] [token]        Intent → Ag screen",
        ));
        self.logs.push(Line::from(
            "  /inspect <0x>              Intent → browse address",
        ));
        self.logs.push(Line::from(
            "  /revoke                    Intent → Approvals manager",
        ));
        self.logs.push(Line::from(
            "  /stealth receive           Intent → Receive (stealth URI)",
        ));
        self.logs.push(Line::from(
            "  sign-typed <json|@file>    EIP-712 sign → Approve card (browserless)",
        ));
        self.logs.push(Line::from(
            "  clear                      Clear console output",
        ));
        self.logs.push(Line::from(
            "  help                       Show this help menu",
        ));
    }

    /// Split trailing `value <wei>` from args; default value `"0"`.
    fn split_value_suffix(parts: &[&str]) -> Result<(Vec<String>, String), String> {
        if parts.is_empty() {
            return Ok((Vec::new(), "0".into()));
        }
        let last = parts.len() - 1;
        if last >= 1 && parts[last - 1].eq_ignore_ascii_case("value") {
            let wei = parts[last].to_string();
            if wei.parse::<alloy::primitives::U256>().is_err() {
                return Err(format!("invalid value wei: {wei}"));
            }
            let args: Vec<String> = parts[..last - 1].iter().map(|s| s.to_string()).collect();
            return Ok((args, wei));
        }
        Ok((parts.iter().map(|s| s.to_string()).collect(), "0".into()))
    }

    fn cmd_write(&mut self, parts: &[&str], wallet: &WalletState) -> KeyOutcome {
        if parts.is_empty() {
            self.log_error("Usage: write <fn_name> [args...] [value <wei>]");
            return KeyOutcome::Consumed;
        }
        let target = match self.active_address {
            Some(a) => a,
            None => {
                self.log_error("No target contract. Use `browse <0xAddress>` first.");
                return KeyOutcome::Consumed;
            }
        };
        let abi = match &self.current_inspection {
            Some(insp) => match &insp.abi_resolution {
                AbiResolution::Verified(abi) => abi.clone(),
                _ => {
                    self.log_error(
                        "Verified ABI required for `write`. Use `writeraw <0xCalldata>` instead.",
                    );
                    return KeyOutcome::Consumed;
                }
            },
            None => {
                self.log_error("Contract not inspected. Run `browse <0xAddress>` first.");
                return KeyOutcome::Consumed;
            }
        };

        let (mut args, value) = match Self::split_value_suffix(parts) {
            Ok(v) => v,
            Err(e) => {
                self.log_error(&e);
                return KeyOutcome::Consumed;
            }
        };
        if args.is_empty() {
            self.log_error("Usage: write <fn_name> [args...] [value <wei>]");
            return KeyOutcome::Consumed;
        }
        let func_name = args.remove(0);
        let calldata = match BrowserEngine::encode_named(&abi, &func_name, &args) {
            Ok(c) => c,
            Err(e) => {
                self.log_error(&format!("Encode failed: {e}"));
                return KeyOutcome::Consumed;
            }
        };
        self.begin_write_confirm(
            wallet,
            target,
            &format!("0x{}", hex::encode(&calldata)),
            &value,
            &format!("write {func_name}({})", args.join(" ")),
        )
    }

    fn cmd_write_raw(&mut self, parts: &[&str], wallet: &WalletState) -> KeyOutcome {
        let target = match self.active_address {
            Some(a) => a,
            None => {
                self.log_error("No target contract. Use `browse <0xAddress>` first.");
                return KeyOutcome::Consumed;
            }
        };
        let (args, value) = match Self::split_value_suffix(parts) {
            Ok(v) => v,
            Err(e) => {
                self.log_error(&e);
                return KeyOutcome::Consumed;
            }
        };
        if args.len() != 1 {
            self.log_error("Usage: writeraw <0xHexCalldata> [value <wei>]");
            return KeyOutcome::Consumed;
        }
        let calldata_hex = &args[0];
        let clean = calldata_hex
            .trim()
            .strip_prefix("0x")
            .unwrap_or(calldata_hex.trim());
        if hex::decode(clean).is_err() {
            self.log_error("Invalid hex calldata");
            return KeyOutcome::Consumed;
        }
        self.begin_write_confirm(wallet, target, &format!("0x{clean}"), &value, "writeraw")
    }

    fn begin_write_confirm(
        &mut self,
        wallet: &WalletState,
        target: Address,
        data_hex: &str,
        value: &str,
        summary: &str,
    ) -> KeyOutcome {
        let from = match wallet.active_address() {
            Ok(a) => a,
            Err(e) => {
                self.log_error(&e.user_message());
                return KeyOutcome::Consumed;
            }
        };
        let chain_id = wallet.networks().active().chain_id;
        let tx = match TransactionService::new().build_contract_call(
            from.to_string(),
            target.to_checksum(None),
            data_hex,
            value,
            chain_id,
        ) {
            Ok(vaughan_core::chains::ChainTransaction::Evm(evm)) => evm,
            Ok(_) => {
                self.log_error("expected EVM transaction");
                return KeyOutcome::Consumed;
            }
            Err(e) => {
                self.log_error(&e.user_message());
                return KeyOutcome::Consumed;
            }
        };

        self.confirm_lines = vec![
            "Contract write (gated — same signer path as Send)".into(),
            String::new(),
            format!("Action:  {summary}"),
            format!("To:      {}", tx.to),
            format!("From:    {}", tx.from),
            format!("Value:   {} wei", tx.value),
            format!(
                "Data:    {}…",
                tx.data
                    .as_deref()
                    .unwrap_or("0x")
                    .chars()
                    .take(42)
                    .collect::<String>()
            ),
            format!("Chain:   {chain_id}"),
        ];
        self.pending_tx = Some(tx.clone());
        self.pending_fee = None;
        self.stage = Stage::ConfirmWrite;
        self.busy = Busy::EstimatingFee;
        self.status = "Estimating fee…".into();
        self.logs.push(Line::from(vec![
            Span::styled("→ ", Style::default().fg(Color::Cyan)),
            Span::raw("opening write confirm (Enter broadcasts after fee)"),
        ]));
        KeyOutcome::StartJob(UiJob::EstimateEvmFee { tx })
    }

    fn cmd_browse(&mut self, addr_str: &str, wallet: &WalletState, handle: &Handle) {
        let addr: Address = match addr_str.parse() {
            Ok(a) => a,
            Err(e) => {
                self.log_error(&format!("Invalid contract address: {}", e));
                return;
            }
        };

        self.active_address = Some(addr);
        let net = wallet.networks().active();
        let chain_id = net.chain_id;

        let engine = self.engine.clone();
        let inspection = handle.block_on(async {
            match wallet.active_adapter().await {
                Ok(adapter) => adapter
                    .with_provider(|provider| {
                        let eng = engine.clone();
                        async move { Ok(eng.inspect(&provider, chain_id, addr).await) }
                    })
                    .await
                    .ok(),
                Err(_) => None,
            }
        });

        match inspection {
            Some(insp) => {
                self.logs.push(Line::from(vec![
                    Span::raw("✔ Loaded contract: "),
                    Span::styled(
                        addr.to_checksum(None),
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));

                // Print ABI info
                match &insp.abi_resolution {
                    AbiResolution::Verified(abi) => {
                        let fn_count = abi.functions.len();
                        self.logs.push(Line::from(vec![
                            Span::styled("  ABI: ", Style::default().fg(Color::DarkGray)),
                            Span::styled("Verified", Style::default().fg(Color::Green)),
                            Span::raw(format!(" ({} callable functions)", fn_count)),
                        ]));

                        // List top function names
                        let mut names: Vec<_> = abi.functions.keys().map(|k| k.as_str()).collect();
                        names.sort_unstable();
                        let preview = names.iter().take(8).copied().collect::<Vec<_>>().join(", ");
                        let more = if names.len() > 8 {
                            format!(" ... (+{} more)", names.len() - 8)
                        } else {
                            String::new()
                        };
                        self.logs
                            .push(Line::from(format!("  Functions: {}{}", preview, more)));
                    }
                    AbiResolution::Unverified => {
                        self.logs.push(Line::from(vec![
                            Span::styled("  ABI: ", Style::default().fg(Color::DarkGray)),
                            Span::styled("Unverified", Style::default().fg(Color::Yellow)),
                            Span::raw(format!(
                                " ({} candidate selectors)",
                                insp.candidate_selectors.len()
                            )),
                        ]));
                        if !insp.candidate_selectors.is_empty() {
                            let hex_list: Vec<_> = insp
                                .candidate_selectors
                                .iter()
                                .take(6)
                                .map(|s| selector_to_hex(*s))
                                .collect();
                            self.logs
                                .push(Line::from(format!("  Selectors: {}", hex_list.join(", "))));
                        }
                    }
                    AbiResolution::Error(err) => {
                        self.logs.push(Line::from(vec![
                            Span::styled("  ABI: ", Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                format!("Explorer Error ({})", err),
                                Style::default().fg(Color::Red),
                            ),
                        ]));
                    }
                }

                self.current_inspection = Some(insp);
            }
            None => {
                self.log_error("Failed to connect to RPC to inspect contract.");
            }
        }
    }

    fn cmd_probe(&mut self, wallet: &WalletState, handle: &Handle) {
        let addr = match self.active_address {
            Some(a) => a,
            None => {
                self.log_error("No target contract selected. Use `browse <0xAddress>` first.");
                return;
            }
        };

        self.cmd_browse(&addr.to_checksum(None), wallet, handle);
    }

    fn cmd_info(&mut self) {
        let addr = match self.active_address {
            Some(a) => a,
            None => {
                self.log_error("No target contract selected. Use `browse <0xAddress>` first.");
                return;
            }
        };

        self.logs.push(Line::from(format!(
            "Contract Address: {}",
            addr.to_checksum(None)
        )));
        if let Some(insp) = &self.current_inspection {
            self.logs
                .push(Line::from(format!("Chain ID:         {}", insp.chain_id)));
            self.logs.push(Line::from(format!(
                "Fingerprint:      {:?}",
                insp.fingerprint
            )));
        }
    }

    fn cmd_call(
        &mut self,
        func_name: &str,
        args: &[String],
        wallet: &WalletState,
        handle: &Handle,
    ) {
        let target = match self.active_address {
            Some(a) => a,
            None => {
                self.log_error("No target contract selected. Use `browse <0xAddress>` first.");
                return;
            }
        };

        let abi = match &self.current_inspection {
            Some(insp) => match &insp.abi_resolution {
                AbiResolution::Verified(abi) => abi.clone(),
                _ => {
                    self.log_error("Contract ABI is not verified. Use `callraw <0xCalldata>` to execute raw calls.");
                    return;
                }
            },
            None => {
                self.log_error("Contract not inspected. Run `browse <0xAddress>` first.");
                return;
            }
        };

        let engine = self.engine.clone();
        let fn_str = func_name.to_string();
        let args_vec = args.to_vec();

        let result = handle.block_on(async {
            match wallet.active_adapter().await {
                Ok(adapter) => adapter
                    .with_provider(|provider| {
                        let eng = engine.clone();
                        let a = abi.clone();
                        let f = fn_str.clone();
                        let av = args_vec.clone();
                        async move {
                            eng.call_named(&provider, target, &a, &f, &av)
                                .await
                                .map_err(vaughan_core::error::WalletError::RpcError)
                        }
                    })
                    .await
                    .map_err(|e| e.user_message()),
                Err(e) => Err(e.user_message()),
            }
        });

        match result {
            Ok(call_res) => {
                self.logs.push(Line::from(vec![
                    Span::styled(
                        "✔ Result: ",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(if call_res.decoded_values.is_empty() {
                        format!("0x{}", hex::encode(&call_res.raw_output))
                    } else {
                        call_res.decoded_values.join(", ")
                    }),
                ]));
            }
            Err(err) => {
                self.log_error(&format!("Call failed: {}", err));
            }
        }
    }

    fn cmd_call_raw(&mut self, calldata_hex: &str, wallet: &WalletState, handle: &Handle) {
        let target = match self.active_address {
            Some(a) => a,
            None => {
                self.log_error("No target contract selected. Use `browse <0xAddress>` first.");
                return;
            }
        };

        let clean = calldata_hex
            .trim()
            .strip_prefix("0x")
            .unwrap_or(calldata_hex.trim());
        let bytes = match hex::decode(clean) {
            Ok(b) => alloy::primitives::Bytes::from(b),
            Err(e) => {
                self.log_error(&format!("Invalid hex calldata: {}", e));
                return;
            }
        };

        let engine = self.engine.clone();

        let result = handle.block_on(async {
            match wallet.active_adapter().await {
                Ok(adapter) => adapter
                    .with_provider(|provider| {
                        let eng = engine.clone();
                        let b = bytes.clone();
                        async move {
                            eng.call_raw(&provider, target, b)
                                .await
                                .map_err(vaughan_core::error::WalletError::RpcError)
                        }
                    })
                    .await
                    .map_err(|e| e.user_message()),
                Err(e) => Err(e.user_message()),
            }
        });

        match result {
            Ok(output) => {
                self.logs.push(Line::from(vec![
                    Span::styled(
                        "✔ Raw Output: ",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("0x{}", hex::encode(&output))),
                ]));
            }
            Err(err) => {
                self.log_error(&format!("Raw call failed: {}", err));
            }
        }
    }

    fn cmd_pairs(&mut self, start: u64, limit: u64, wallet: &WalletState, handle: &Handle) {
        let target = match self.active_address {
            Some(a) => a,
            None => {
                self.log_error("No factory selected. Use `browse <0xFactoryAddress>` first.");
                return;
            }
        };

        let result = handle.block_on(async {
            match wallet.active_adapter().await {
                Ok(adapter) => adapter
                    .with_provider(|provider| async move {
                        let count = PairDiscovery::get_v2_pairs_count(&provider, target)
                            .await
                            .unwrap_or(0);
                        let pairs =
                            PairDiscovery::fetch_v2_pairs_range(&provider, target, start, limit)
                                .await;
                        Ok((count, pairs))
                    })
                    .await
                    .map_err(|e| e.user_message()),
                Err(e) => Err(e.user_message()),
            }
        });

        match result {
            Ok((total, pairs)) => {
                self.logs
                    .push(Line::from(format!("Factory Total Pairs: {}", total)));
                if pairs.is_empty() {
                    self.logs
                        .push(Line::from("  No pairs found in index range."));
                } else {
                    for (i, p) in pairs.iter().enumerate() {
                        self.logs.push(Line::from(format!(
                            "  [{}] {}",
                            start + (i as u64),
                            p.pair_address.to_checksum(None)
                        )));
                    }
                }
            }
            Err(err) => {
                self.log_error(&format!("Failed to query factory pairs: {}", err));
            }
        }
    }

    fn log_error(&mut self, err: &str) {
        self.logs.push(Line::from(vec![
            Span::styled(
                "✖ Error: ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(err.to_string(), Style::default().fg(Color::Red)),
        ]));
    }
}

fn load_typed_data_for_repl(raw: &str) -> Result<serde_json::Value, String> {
    let text = if let Some(path) = raw.strip_prefix('@') {
        std::fs::read_to_string(path.trim()).map_err(|e| format!("read file: {e}"))?
    } else {
        raw.to_string()
    };
    let v: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("typed-data JSON: {e}"))?;
    if !v.is_object() {
        return Err("typed-data must be a JSON object".into());
    }
    for key in ["types", "domain", "primaryType", "message"] {
        if v.get(key).is_none() {
            return Err(format!("typed-data missing `{key}`"));
        }
    }
    Ok(v)
}
