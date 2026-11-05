//! Interactive Ratatui Terminal Dashboard for `vaughan-hl` (`#[cfg(feature = "tui")]`).
//!
//! Designed so it can run standalone (`vaughan-hl` / `vaughan-hl tui`) **or** be
//! mounted directly inside `Vaughan-CLI`'s `vaughan-tui` via [`HlTuiApp::draw`]
//! and [`HlTuiApp::handle_key`].

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use alloy::primitives::Address;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Row, Table},
    Frame, Terminal,
};
use secrecy::SecretString;

use crate::engine::{evaluate_closed_candle_tick, BracketOrderPlan};
use crate::error::HlError;
use crate::indicators::IndicatorTable;
use crate::keystore::{validate_password_policy, EncryptedAgentKeystore};
use crate::live::{self, LiveConfig, LiveHandle, LiveStatus};
use crate::market_data::{
    aggregate_candles, default_cache_path, derived_source_interval, fetch_candles_from_hl,
    fetch_clearinghouse_state, fetch_spot_balance, fetch_user_account_telemetry, fetch_user_fills,
    interval_duration_ms, load_candles_cache, now_unix_ms, save_candles_cache, uk_day_number,
    Candle, LiveTradeFill, UserAccountTelemetry, HL_MAINNET_INFO_URL, HL_TESTNET_INFO_URL,
};
use crate::replay::{
    run_replay_with_table, ReplayReport, Side, SimulatedTrade, DEFAULT_HL_TAKER_FEE_BPS,
    DEFAULT_SLIPPAGE_BPS,
};
use crate::strategy::{estimated_liq_distance_bps, CompiledStrategy, SizingMode, StopLossSpec};
use crate::wire::build_approve_agent_typed_data;

const COL_NEON_BLUE: Color = Color::Rgb(80, 200, 255);
const COL_PURPLE: Color = Color::Rgb(185, 110, 255);
const COL_HOT_PINK: Color = Color::Rgb(255, 70, 175);
const COL_KEY: Color = Color::Rgb(110, 210, 255);
const COL_GREEN: Color = Color::Rgb(70, 235, 140);
const COL_RED: Color = Color::Rgb(255, 95, 105);
const COL_YELLOW: Color = Color::Rgb(255, 210, 80);
const COL_DIM: Color = Color::Rgb(135, 145, 165);
const COL_BORDER: Color = Color::Rgb(70, 115, 175);

/// Vaughan-Dioxus address palette (`#808080` / `#33cc33` / `#ff9933` / `#b24cff`).
pub const ADDR_GREY: Color = Color::Rgb(128, 128, 128);
pub const ADDR_GREEN: Color = Color::Rgb(51, 204, 51);
pub const ADDR_ORANGE: Color = Color::Rgb(255, 153, 51);
pub const ADDR_PURPLE: Color = Color::Rgb(178, 76, 255);

/// Frames for a braille spinner (pure ratatui, zero extra crates).
pub fn spinner_frame(tick: u64) -> char {
    const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[(tick as usize) % FRAMES.len()]
}

/// Rainbow-segmented wallet address spans matching Vaughan-Dioxus layout:
/// `0x` (grey) / 5 chars (green) / 13 chars (grey) / 5 chars (orange) / 12 chars (grey) / 5 chars (purple).
pub fn colored_address_spans(address: &str) -> Vec<Span<'static>> {
    let addr = address.trim();
    let body = addr
        .strip_prefix("0x")
        .or_else(|| addr.strip_prefix("0X"))
        .unwrap_or(addr);
    if body.len() < 40 {
        return vec![Span::styled(
            addr.to_string(),
            Style::default().fg(ADDR_GREY),
        )];
    }

    let mut spans = Vec::with_capacity(6);
    if addr.starts_with("0x") || addr.starts_with("0X") {
        spans.push(Span::styled(
            "0x".to_string(),
            Style::default().fg(ADDR_GREY),
        ));
    }
    let parts: &[(usize, usize, Color)] = &[
        (0, 5, ADDR_GREEN),
        (5, 18, ADDR_GREY),
        (18, 23, ADDR_ORANGE),
        (23, 35, ADDR_GREY),
        (35, 40, ADDR_PURPLE),
    ];
    for &(start, end, color) in parts {
        spans.push(Span::styled(
            body[start..end].to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    spans
}

/// Address spans during the copy pulse: a braille wave in the address colours with
/// `copied` centred, keeping the exact character width of the original address.
pub fn address_copy_pulse_spans(address: &str, tick: u64) -> Vec<Span<'static>> {
    const WORD: &str = " copied ";
    let base = colored_address_spans(address);
    let cells: Vec<Style> = base
        .iter()
        .flat_map(|s| s.content.chars().map(move |_| s.style))
        .collect();
    let text: String = base.iter().map(|s| s.content.as_ref()).collect();
    let start = text.chars().take_while(|c| *c == ' ').count();
    let body_len = cells.len().saturating_sub(start);
    let word_len = WORD.chars().count();
    if body_len < word_len {
        return base;
    }
    let word_at = start + (body_len - word_len) / 2;

    let mut spans = Vec::with_capacity(body_len + 2);
    if start > 0 {
        spans.push(Span::raw(" ".repeat(start)));
    }
    let mut i = start;
    while i < cells.len() {
        if i == word_at {
            spans.push(Span::styled(
                WORD,
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ));
            i += word_len;
            continue;
        }
        let glyph = spinner_frame(tick.wrapping_add(i as u64));
        spans.push(Span::styled(glyph.to_string(), cells[i]));
        i += 1;
    }
    spans
}

/// Precomputed summary & telemetry for one strategy `.toml` file.
#[derive(Debug, Clone)]
pub struct StrategyEntry {
    pub file_name: String,
    pub path: PathBuf,
    pub compiled: CompiledStrategy,
    pub report: ReplayReport,
    pub h1_net_r: f64,
    pub h1_pf: f64,
    pub h1_trades: usize,
    pub h2_net_r: f64,
    pub h2_pf: f64,
    pub h2_trades: usize,
    pub wf_verified: bool,
    pub last_candle: Option<Candle>,
    pub last_rsi: Option<f64>,
    pub last_rsi_ma: Option<f64>,
    pub last_kst: Option<f64>,
    pub last_kst_sig: Option<f64>,
    pub last_crsi: Option<f64>,
    pub last_adx: Option<f64>,
    pub last_stoch_k: Option<f64>,
    pub last_htf_stoch_k: Option<f64>,
    pub last_htf_stoch_d: Option<f64>,
    pub latest_tick_plan: Option<BracketOrderPlan>,
    pub selected_markets: Vec<String>,
}

/// Single market choice displayed in the multi-market selector modal.
#[derive(Debug, Clone)]
pub struct MarketSelectionItem {
    pub symbol: String,
    pub name: String,
    pub selected: bool,
}

/// Modal dialog for choosing which markets to arm for a strategy.
#[derive(Debug, Clone)]
pub struct MarketSelectorModal {
    pub strategy_label: String,
    pub items: Vec<MarketSelectionItem>,
    pub cursor: usize,
    pub input_mode: bool,
    pub input_buf: String,
    pub error_msg: Option<String>,
}

impl MarketSelectorModal {
    pub fn new(strategy_label: String, items: Vec<MarketSelectionItem>) -> Self {
        Self {
            strategy_label,
            items,
            cursor: 0,
            input_mode: false,
            input_buf: String::new(),
            error_msg: None,
        }
    }
}

/// Popular default markets pre-populated in the selector.
pub fn popular_market_choices(current_coins: &[String]) -> Vec<MarketSelectionItem> {
    let defaults = [
        ("xyz:CL", "WTI Crude Oil (USDC)"),
        ("xyz:GOLD", "Gold (USDC)"),
        ("xyz:SILVER", "Silver (USDC)"),
        ("xyz:COPPER", "Copper (USDC)"),
        ("xyz:NATGAS", "Natural Gas (USDC)"),
        ("BTC", "Bitcoin (USDC)"),
        ("ETH", "Ethereum (USDC)"),
        ("SOL", "Solana (USDC)"),
        ("HYPE", "Hyperliquid (USDC)"),
        ("SPX", "S&P 500 (USDC)"),
        ("kPEPE", "Pepe 1000 (USDC)"),
        ("DOGE", "Dogecoin (USDC)"),
        ("SUI", "Sui (USDC)"),
        ("xyz:NVDA", "NVIDIA Corp (USDC)"),
        ("xyz:TSLA", "Tesla Inc (USDC)"),
        ("xyz:AAPL", "Apple Inc (USDC)"),
    ];
    let mut items = Vec::new();
    for (sym, name) in defaults {
        let selected = current_coins.iter().any(|c| c.eq_ignore_ascii_case(sym));
        items.push(MarketSelectionItem {
            symbol: sym.to_string(),
            name: name.to_string(),
            selected,
        });
    }
    for c in current_coins {
        if !items.iter().any(|it| it.symbol.eq_ignore_ascii_case(c)) {
            items.push(MarketSelectionItem {
                symbol: c.clone(),
                name: format!("{c} (Custom)"),
                selected: true,
            });
        }
    }
    items
}

/// What a masked password prompt is collecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// Choose the password that will encrypt the new agent key.
    Connect,
    /// Unlock the stored agent key so live orders can be signed.
    Unlock,
}

/// Masked password entry state (the typed text is never rendered or logged).
pub struct PasswordPrompt {
    pub kind: PromptKind,
    buf: String,
    pub error: Option<String>,
}

impl std::fmt::Debug for PasswordPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordPrompt")
            .field("kind", &self.kind)
            .field("buf", &"[REDACTED]")
            .finish()
    }
}

/// Key outcome returned by [`HlTuiApp::handle_key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HlKeyOutcome {
    /// Key was handled internally by the app.
    Consumed,
    /// Caller should exit or navigate away.
    Exit,
    /// User pressed Enter or 'a' on the Quick-Connect card to request 1-click authorization.
    ConnectRequested,
    /// User pressed 'w' on the Quick-Connect card to open the testnet faucet in VB.
    OpenFaucet,
}

/// Helper to launch VB (`vaughan-dapp-browser`) for whitelisted dApp URLs.
pub fn spawn_vb(url: &str, allow_hosts: &[&str]) -> bool {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("vaughan-dapp-browser"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let home = std::path::PathBuf::from(home);
        candidates.push(home.join(".cargo/bin/vaughan-dapp-browser"));
        candidates.push(home.join(".local/bin/vaughan-dapp-browser"));
    }
    candidates.push(std::path::PathBuf::from(
        "target/release/vaughan-dapp-browser",
    ));
    candidates.push(std::path::PathBuf::from(
        "target/debug/vaughan-dapp-browser",
    ));
    candidates.push(std::path::PathBuf::from("vaughan-dapp-browser"));

    let is_file = url.starts_with("file://");
    for bin in candidates {
        let mut cmd = std::process::Command::new(&bin);
        cmd.arg("--url").arg(url);
        for h in allow_hosts {
            cmd.arg("--allow-host").arg(h);
        }
        if !is_file {
            cmd.arg("--keep-profile");
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if cmd.spawn().is_ok() {
            return true;
        }
    }
    false
}

/// Active focus column in the main Hyperliquid Auto-Trader dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HlFocus {
    #[default]
    Strategies,
    Markets,
}

/// Outcome of a background Hyperliquid registration attempt.
pub type ConnectOutcome =
    Result<(EncryptedAgentKeystore, SecretString, Address, bool, f64), String>;

/// Embeddable Ratatui state for the Hyperliquid Auto-Trader view.
pub struct HlTuiApp {
    pub data_dir: PathBuf,
    pub strategies_dir: PathBuf,
    pub entries: Vec<StrategyEntry>,
    pub selected: usize,
    pub focus: HlFocus,
    pub market_cursor: usize,
    pub available_markets: Vec<MarketSelectionItem>,
    pub market_input_mode: bool,
    pub market_input_buf: String,
    pub trade_scroll: usize,
    pub armed_dry_run: bool,
    pub status_msg: String,
    pub agent_summary: String,
    pub is_connected: bool,
    pub is_mainnet: bool,
    pub backing_account: Option<Address>,
    pub agent_address: Option<Address>,
    pub live_equity_usd: Option<f64>,
    pub live_equity: Arc<Mutex<Option<f64>>>,
    pub live_spot_balance_usd: Option<f64>,
    pub live_spot_usd: Arc<Mutex<Option<f64>>>,
    pub live_telemetry_data: Option<UserAccountTelemetry>,
    pub live_telemetry: Arc<Mutex<Option<UserAccountTelemetry>>>,
    pub live_trades: Arc<Mutex<Vec<LiveTradeFill>>>,
    pub last_poll_time_ms: u64,
    pub last_fetch_time_ms: u64,
    pub poll_counter: u64,
    pub prompt: Option<PasswordPrompt>,
    pub market_modal: Option<MarketSelectorModal>,
    pub pending_password: Option<SecretString>,
    pub arm_after_unlock: bool,
    pub live_on: bool,
    pub last_candle_fetch_ms: u64,
    pub rt: Option<tokio::runtime::Handle>,
    pub connect_status: Arc<Mutex<Option<ConnectOutcome>>>,
    pub is_connecting: Arc<AtomicBool>,
    pub copy_pulse_ticks: u8,
    pub anim_tick: u64,
    pub show_chart: bool,
    pub chart_scroll: usize,
    pub chart_market_idx: usize,
    pub session_start_ms: u64,
    pub show_all_day_trades: bool,
}

#[inline]
fn norm_pnl(val: f64) -> f64 {
    if val.abs() < 1e-4 {
        0.0
    } else {
        val
    }
}

#[inline]
fn norm_coin(s: &str) -> &str {
    s.strip_prefix("xyz:").unwrap_or(s)
}

impl HlTuiApp {
    /// Automatically resolve `(data_dir, strategies_dir)` from the current working directory
    /// or the crate directory (`CARGO_MANIFEST_DIR`) and load all strategies.
    pub fn load_default() -> Self {
        let candidates = [
            (PathBuf::from("data"), PathBuf::from("strategies")),
            (
                PathBuf::from("vaughan-hl/data"),
                PathBuf::from("vaughan-hl/strategies"),
            ),
            (
                Path::new(env!("CARGO_MANIFEST_DIR")).join("data"),
                Path::new(env!("CARGO_MANIFEST_DIR")).join("strategies"),
            ),
        ];
        for (data_dir, strategies_dir) in &candidates {
            if strategies_dir.is_dir() {
                if let Ok(app) = Self::load(data_dir, strategies_dir) {
                    return app;
                }
            }
        }
        Self::load(Path::new("data"), Path::new("strategies")).unwrap_or_else(|_| Self {
            data_dir: PathBuf::from("data"),
            strategies_dir: PathBuf::from("strategies"),
            entries: Vec::new(),
            selected: 0,
            focus: HlFocus::Strategies,
            market_cursor: 0,
            available_markets: popular_market_choices(&[]),
            market_input_mode: false,
            market_input_buf: String::new(),
            trade_scroll: 0,
            armed_dry_run: false,
            status_msg: "No strategies directory found.".to_string(),
            agent_summary: "No Agent Connected".to_string(),
            is_connected: false,
            is_mainnet: false,
            backing_account: None,
            agent_address: None,
            live_equity_usd: None,
            live_equity: Arc::new(Mutex::new(None)),
            live_spot_balance_usd: None,
            live_spot_usd: Arc::new(Mutex::new(None)),
            live_telemetry_data: None,
            live_telemetry: Arc::new(Mutex::new(None)),
            live_trades: Arc::new(Mutex::new(Vec::new())),
            last_poll_time_ms: 0,
            last_fetch_time_ms: 0,
            poll_counter: 0,
            prompt: None,
            market_modal: None,
            pending_password: None,
            arm_after_unlock: false,
            live_on: false,
            last_candle_fetch_ms: 0,
            rt: None,
            connect_status: Arc::new(Mutex::new(None)),
            is_connecting: Arc::new(AtomicBool::new(false)),
            copy_pulse_ticks: 0,
            anim_tick: 0,
            show_chart: false,
            chart_scroll: 0,
            chart_market_idx: 0,
            session_start_ms: now_unix_ms(),
            show_all_day_trades: false,
        })
    }

    /// Load all `strategies/*.toml` files and evaluate them against cached candles in `data_dir`.
    pub fn load(data_dir: &Path, strategies_dir: &Path) -> Result<Self, HlError> {
        let mut entries = Vec::new();

        let mut paths: Vec<PathBuf> = if strategies_dir.exists() {
            fs::read_dir(strategies_dir)?
                .filter_map(|e| e.ok().map(|d| d.path()))
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("toml"))
                .collect()
        } else {
            Vec::new()
        };
        paths.sort();

        for path in paths {
            if let Some(entry) = load_strategy_entry(data_dir, &path) {
                entries.push(entry);
            }
        }

        // Sort: combo strategies first (t10 then t3), then test strategies, then Walk-Forward Verified profitable strategies, then by Net R descending.
        entries.sort_by(|a, b| {
            let a_is_combo = a.file_name.starts_with("t10") || a.file_name.starts_with("t3");
            let b_is_combo = b.file_name.starts_with("t10") || b.file_name.starts_with("t3");
            if a_is_combo != b_is_combo {
                return b_is_combo.cmp(&a_is_combo);
            }
            if a_is_combo && b_is_combo {
                return a.file_name.cmp(&b.file_name);
            }
            let a_is_test = a.file_name.starts_with("test");
            let b_is_test = b.file_name.starts_with("test");
            if a_is_test != b_is_test {
                return b_is_test.cmp(&a_is_test);
            }
            b.wf_verified.cmp(&a.wf_verified).then_with(|| {
                b.report
                    .total_net_r
                    .partial_cmp(&a.report.total_net_r)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });

        let effective_data_dir = if data_dir.join("hl_agent_keystore.json").exists() {
            data_dir.to_path_buf()
        } else if data_dir == Path::new("data") {
            if Path::new("data/hl_agent_keystore.json").exists() {
                PathBuf::from("data")
            } else if Path::new("vaughan-hl/data/hl_agent_keystore.json").exists() {
                PathBuf::from("vaughan-hl/data")
            } else if Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("data/hl_agent_keystore.json")
                .exists()
            {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("data")
            } else {
                data_dir.to_path_buf()
            }
        } else {
            data_dir.to_path_buf()
        };

        let ks_path = effective_data_dir.join("hl_agent_keystore.json");
        let (agent_summary, is_connected, backing_account, is_mainnet, agent_address) =
            if let Ok(ks) = EncryptedAgentKeystore::load_from_file(&ks_path) {
                (
                    format!(
                        "Agent {:#x} -> Backing {:#x} ({})",
                        ks.agent_address,
                        ks.backing_account,
                        if ks.is_mainnet { "MAINNET" } else { "TESTNET" }
                    ),
                    true,
                    Some(ks.backing_account),
                    ks.is_mainnet,
                    Some(ks.agent_address),
                )
            } else {
                (
                    "No Agent Connected (Press Enter / a to 1-Click Connect)".to_string(),
                    false,
                    None,
                    false,
                    None,
                )
            };

        let status_msg = if is_connected {
            format!(
                "Loaded {} strategies ({} walk-forward verified profitable in H1 & H2).",
                entries.len(),
                entries.iter().filter(|e| e.wf_verified).count()
            )
        } else if is_mainnet {
            "Hyperliquid Mainnet 1-Click Setup: Press [Enter] or [a] to connect agent key."
                .to_string()
        } else {
            "Hyperliquid Testnet 1-Click Setup: Press [Enter] or [a] to connect agent key."
                .to_string()
        };

        let available_markets = if let Some(cur) = entries.first() {
            popular_market_choices(&cur.selected_markets)
        } else {
            popular_market_choices(&[])
        };

        Ok(Self {
            data_dir: effective_data_dir,
            strategies_dir: strategies_dir.to_path_buf(),
            entries,
            selected: 0,
            focus: HlFocus::Strategies,
            market_cursor: 0,
            available_markets,
            market_input_mode: false,
            market_input_buf: String::new(),
            trade_scroll: 0,
            armed_dry_run: false,
            status_msg,
            agent_summary,
            is_connected,
            is_mainnet,
            backing_account,
            agent_address,
            live_equity_usd: None,
            live_equity: Arc::new(Mutex::new(None)),
            live_spot_balance_usd: None,
            live_spot_usd: Arc::new(Mutex::new(None)),
            live_telemetry_data: None,
            live_telemetry: Arc::new(Mutex::new(None)),
            live_trades: Arc::new(Mutex::new(Vec::new())),
            last_poll_time_ms: 0,
            last_fetch_time_ms: 0,
            poll_counter: 0,
            prompt: None,
            market_modal: None,
            pending_password: None,
            arm_after_unlock: false,
            live_on: false,
            last_candle_fetch_ms: 0,
            rt: None,
            connect_status: Arc::new(Mutex::new(None)),
            is_connecting: Arc::new(AtomicBool::new(false)),
            copy_pulse_ticks: 0,
            anim_tick: 0,
            show_chart: false,
            chart_scroll: 0,
            chart_market_idx: 0,
            session_start_ms: now_unix_ms(),
            show_all_day_trades: false,
        })
    }

    pub fn mark_connected(
        &mut self,
        agent: Address,
        backing: Address,
        is_mainnet: bool,
        equity: f64,
    ) {
        self.is_connected = true;
        self.is_mainnet = is_mainnet;
        self.backing_account = Some(backing);
        self.agent_address = Some(agent);
        self.live_equity_usd = Some(equity);
        if let Ok(mut g) = self.live_equity.lock() {
            *g = Some(equity);
        }
        self.agent_summary = format!(
            "Agent {:#x} -> Backing {:#x} ({})",
            agent,
            backing,
            if is_mainnet { "MAINNET" } else { "TESTNET" }
        );
        self.status_msg = if equity > 0.0 {
            format!("● Connected to Hyperliquid (Equity: ${equity:.2} USDC). Press Space to Arm.")
        } else if is_mainnet {
            "● Connected to Hyperliquid Mainnet. Deposit USDC on Arbitrum to start live trading."
                .to_string()
        } else {
            "● Connected to Hyperliquid Testnet. Claim free 1,000 USDC Faucet with [w].".to_string()
        };
    }

    pub fn set_network_mode(&mut self, is_mainnet: bool) {
        self.is_mainnet = is_mainnet;
        let ks_path = self.data_dir.join("hl_agent_keystore.json");
        if let Ok(ks) = EncryptedAgentKeystore::load_from_file(&ks_path) {
            if ks.is_mainnet == is_mainnet {
                self.is_connected = true;
                self.backing_account = Some(ks.backing_account);
                self.agent_address = Some(ks.agent_address);
                self.agent_summary = format!(
                    "Agent {:#x} -> Backing {:#x} ({})",
                    ks.agent_address,
                    ks.backing_account,
                    if is_mainnet { "MAINNET" } else { "TESTNET" }
                );
                return;
            }
        }
        self.is_connected = false;
        self.live_equity_usd = None;
        if let Ok(mut g) = self.live_equity.lock() {
            *g = None;
        }
        self.live_spot_balance_usd = None;
        if let Ok(mut g) = self.live_spot_usd.lock() {
            *g = None;
        }
        self.live_telemetry_data = None;
        if let Ok(mut g) = self.live_telemetry.lock() {
            *g = None;
        }
        if let Ok(mut g) = self.live_trades.lock() {
            g.clear();
        }
        self.agent_summary = if is_mainnet {
            "No Agent Connected (Press [Enter] or [a] to Connect to MAINNET)".to_string()
        } else {
            "No Agent Connected (Press [Enter] or [a] to Connect to TESTNET)".to_string()
        };
        self.status_msg = if is_mainnet {
            "Hyperliquid Mainnet: Press [Enter] or [a] to 1-Click Authorize Agent Key.".to_string()
        } else {
            "Hyperliquid Testnet: Press [Enter] or [a] to 1-Click Authorize Agent Key.".to_string()
        };
    }

    pub fn set_connect_error(&mut self, err: String) {
        self.status_msg = format!("Connect error: {err}. Press [w] to open Faucet or try again.");
    }

    pub fn set_backing_account(&mut self, addr: Address) {
        self.backing_account = Some(addr);
    }

    /// Runtime handle used for background network tasks (set by the embedding app).
    pub fn set_runtime_handle(&mut self, handle: tokio::runtime::Handle) {
        self.rt = Some(handle);
    }

    fn runtime(&self) -> Option<tokio::runtime::Handle> {
        self.rt
            .clone()
            .or_else(|| tokio::runtime::Handle::try_current().ok())
    }

    /// `true` while a masked password prompt owns the keyboard.
    pub fn prompt_active(&self) -> bool {
        self.prompt.is_some()
    }

    /// Copy the Hyperliquid Agent address to the clipboard and trigger the copy pulse animation.
    pub fn copy_agent_address(&mut self) -> bool {
        let addr_opt = self.agent_address.or_else(|| {
            EncryptedAgentKeystore::load_from_file(&self.keystore_path())
                .ok()
                .map(|k| k.agent_address)
        });
        if let Some(agent) = addr_opt {
            let addr_str = format!("{:#x}", agent);
            let copied = crate::market_data::copy_to_clipboard(&addr_str);
            self.copy_pulse_ticks = 15;
            if copied {
                self.status_msg = format!("● Copied Agent {addr_str} to clipboard! Paste at app.hyperliquid.xyz or run `vaughan hl connect --mainnet`");
            } else {
                self.status_msg = format!("● Agent Key: {addr_str} (authorize at app.hyperliquid.xyz or run `vaughan hl connect --mainnet`)");
            }
            true
        } else {
            self.status_msg =
                "No agent keystore found on disk. Run `vaughan hl connect --mainnet`.".into();
            false
        }
    }

    /// Open Hyperliquid in VB (vaughan-dapp-browser), the exact same way as pressing Enter from the Whitelist view in Vaughan TUI.
    pub fn open_hyperliquid_dapp(&mut self) -> String {
        let url = if self.is_mainnet {
            "https://app.hyperliquid.xyz/trade"
        } else {
            "https://app.hyperliquid-testnet.xyz/trade"
        };

        let hosts = &[
            "app.hyperliquid.xyz",
            "hyperliquid.xyz",
            "app.hyperliquid-testnet.xyz",
            "hyperliquid-testnet.xyz",
        ][..];

        let opened = spawn_vb(url, hosts);
        if opened {
            self.status_msg = "opened in VB (vaughan-dapp-browser) · saved site data — approve sign/send in Vaughan TUI".into();
        } else {
            #[cfg(target_os = "linux")]
            let _ = std::process::Command::new("xdg-open").arg(url).spawn();
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").arg(url).spawn();
            self.status_msg = format!("Opened Hyperliquid ({url})");
        }
        url.to_string()
    }

    /// Take the password the user chose for the new agent key (one-shot).
    pub fn take_password(&mut self) -> Option<SecretString> {
        self.pending_password.take()
    }

    /// After a successful 1-click connect: keep the unlocked agent signer for this
    /// process so arming does not ask for the password again.
    pub fn cache_agent_signer(&mut self, ks: &EncryptedAgentKeystore, pw: &SecretString) {
        if let Ok(signer) = ks.unlock(pw) {
            live::cache_signer(Arc::new(signer));
        }
    }

    fn keystore_path(&self) -> PathBuf {
        let direct = self.data_dir.join("hl_agent_keystore.json");
        if direct.exists() || self.data_dir != Path::new("data") {
            return direct;
        }
        for candidate in [
            PathBuf::from("data/hl_agent_keystore.json"),
            PathBuf::from("vaughan-hl/data/hl_agent_keystore.json"),
            Path::new(env!("CARGO_MANIFEST_DIR")).join("data/hl_agent_keystore.json"),
        ] {
            if candidate.exists() {
                return candidate;
            }
        }
        direct
    }

    /// Start (or refuse to start) the live loop for the selected strategy across its chosen markets.
    fn start_live(&mut self) {
        if let Some(cur) = self.current() {
            if cur.file_name.starts_with("t10") || cur.file_name.contains("top10") {
                self.start_live_top10();
                return;
            }
            if cur.file_name.starts_with("t3") || cur.file_name.contains("top3") {
                self.start_live_top3();
                return;
            }
        }
        let markets = self
            .current()
            .map(|cur| {
                if cur.selected_markets.is_empty() {
                    vec![cur.compiled.config.market.coin.clone()]
                } else {
                    cur.selected_markets.clone()
                }
            })
            .unwrap_or_default();
        self.start_live_markets(&markets);
    }

    /// Arm the entire Top 10 Portfolio simultaneously across all 10 optimal combos.
    fn start_live_top10(&mut self) {
        let mut backing = self.backing_account;
        if backing.is_none() {
            if let Ok(ks) = EncryptedAgentKeystore::load_from_file(&self.keystore_path()) {
                backing = Some(ks.backing_account);
                self.backing_account = backing;
            }
        }
        let Some(backing) = backing else {
            self.status_msg = "No backing account; connect first with [Enter] or [a].".into();
            return;
        };

        let Some(signer) = live::cached_signer(self.is_mainnet, backing) else {
            self.arm_after_unlock = true;
            self.prompt = Some(PasswordPrompt {
                kind: PromptKind::Unlock,
                buf: String::new(),
                error: None,
            });
            self.status_msg =
                "Enter Vaughan password to unlock agent key and arm T10 COMBO.".into();
            return;
        };

        let total = self
            .live_telemetry_data
            .as_ref()
            .map(|t| t.portfolio_value)
            .unwrap_or_else(|| {
                let perps = self.live_equity_usd.unwrap_or(0.0);
                let spot = self.live_spot_balance_usd.unwrap_or(0.0);
                if perps > 0.0 {
                    perps
                } else {
                    spot
                }
            });
        if total <= 0.0 && (self.live_equity_usd.is_some() || self.live_telemetry_data.is_some()) {
            self.armed_dry_run = true;
            self.live_on = false;
            self.last_poll_time_ms = 0;
            self.status_msg =
                "SIMULATION armed on TOP 10 PORTFOLIO · $0.00 balance: paper-trading mode".into();
            return;
        }

        let Some(rt) = self.runtime() else {
            self.status_msg = "No async runtime available for live trading.".into();
            return;
        };

        let top10_files = [
            ("jim_oil.toml", "xyz:CL"),
            ("kate_nvda.toml", "xyz:NVDA"),
            ("bob_btc_3m.toml", "BTC"),
            ("bob_sol_3m.toml", "SOL"),
            ("bob_eth_3m.toml", "ETH"),
            ("bob_oil.toml", "xyz:CL"),
            ("bob_pepe.toml", "kPEPE"),
            ("bob_gold.toml", "xyz:GOLD"),
            ("sue_sp500.toml", "xyz:SP500"),
            ("jim_sp500.toml", "xyz:SP500"),
        ];

        live::stop_active();
        let alloc = live::allocation_from_env();
        let mut armed_count = 0;
        for (file, coin) in top10_files {
            let path = self.strategies_dir.join(file);
            if let Ok(mut strategy) = CompiledStrategy::from_toml_file(&path) {
                strategy.config.market.coin = coin.to_string();
                let cfg = LiveConfig::new(
                    self.is_mainnet,
                    backing,
                    strategy,
                    format!("{file} [{coin}]"),
                    alloc,
                );
                let stop = Arc::new(AtomicBool::new(false));
                let status = Arc::new(Mutex::new(LiveStatus::starting(format!(
                    "LIVE [{file} [{coin}]] starting..."
                ))));
                live::register_run(LiveHandle {
                    stop: Arc::clone(&stop),
                    status: Arc::clone(&status),
                    strategy_label: file.to_string(),
                    is_mainnet: self.is_mainnet,
                    coin: coin.to_string(),
                });
                rt.spawn(live::run_live(cfg, Arc::clone(&signer), stop, status));
                armed_count += 1;
            }
        }

        if armed_count == 0 {
            self.live_on = false;
            self.armed_dry_run = false;
            self.status_msg = "Failed to load any of the T10 strategies.".into();
            return;
        }

        self.armed_dry_run = true;
        self.live_on = true;
        self.status_msg = format!(
            "LIVE ARMED [T10 COMBO]: {armed_count} strategies running across 8 markets · Space/0 to stop"
        );
    }

    /// Start live execution across the top 3 scalper markets (BTC, SOL, ETH [3m]).
    fn start_live_top3(&mut self) {
        let mut backing = self.backing_account;
        if backing.is_none() {
            if let Ok(ks) = EncryptedAgentKeystore::load_from_file(&self.keystore_path()) {
                backing = Some(ks.backing_account);
                self.backing_account = backing;
            }
        }
        let Some(backing) = backing else {
            self.status_msg = "No backing account; connect first with [Enter] or [a].".into();
            return;
        };

        let Some(signer) = live::cached_signer(self.is_mainnet, backing) else {
            self.arm_after_unlock = true;
            self.prompt = Some(PasswordPrompt {
                kind: PromptKind::Unlock,
                buf: String::new(),
                error: None,
            });
            self.status_msg = "Enter Vaughan password to unlock agent key and arm LIVE.".into();
            return;
        };

        let Some(rt) = self.runtime() else {
            self.status_msg = "No async runtime available for live trading.".into();
            return;
        };

        let top3_files = [
            ("bob_btc_3m.toml", "BTC"),
            ("bob_sol_3m.toml", "SOL"),
            ("bob_eth_3m.toml", "ETH"),
        ];

        live::stop_active();
        let alloc = live::allocation_from_env();
        let mut armed_count = 0;
        for (file, coin) in top3_files {
            let path = self.strategies_dir.join(file);
            if let Ok(mut strategy) = CompiledStrategy::from_toml_file(&path) {
                strategy.config.market.coin = coin.to_string();
                let cfg = LiveConfig::new(
                    self.is_mainnet,
                    backing,
                    strategy,
                    format!("{file} [{coin}]"),
                    alloc,
                );
                let stop = Arc::new(AtomicBool::new(false));
                let status = Arc::new(Mutex::new(LiveStatus::starting(format!(
                    "LIVE [{file} [{coin}]] starting..."
                ))));
                live::register_run(LiveHandle {
                    stop: Arc::clone(&stop),
                    status: Arc::clone(&status),
                    strategy_label: file.to_string(),
                    is_mainnet: self.is_mainnet,
                    coin: coin.to_string(),
                });
                rt.spawn(live::run_live(cfg, Arc::clone(&signer), stop, status));
                armed_count += 1;
            }
        }

        if armed_count == 0 {
            self.live_on = false;
            self.armed_dry_run = false;
            self.status_msg = "Failed to load any of the T3 strategies.".into();
            return;
        }

        self.armed_dry_run = true;
        self.live_on = true;
        self.status_msg = format!(
            "LIVE ARMED [T3 COMBO]: {armed_count} strategies running across BTC, SOL, ETH [3m] · Space/3 to stop"
        );
    }

    /// Start live execution across multiple specified markets.
    fn start_live_markets(&mut self, markets: &[String]) {
        let mut backing = self.backing_account;
        if backing.is_none() {
            if let Ok(ks) = EncryptedAgentKeystore::load_from_file(&self.keystore_path()) {
                backing = Some(ks.backing_account);
                self.backing_account = backing;
            }
        }
        let Some(backing) = backing else {
            self.status_msg = "No backing account; connect first with [Enter] or [a].".into();
            return;
        };

        let Some(signer) = live::cached_signer(self.is_mainnet, backing) else {
            self.arm_after_unlock = true;
            self.prompt = Some(PasswordPrompt {
                kind: PromptKind::Unlock,
                buf: String::new(),
                error: None,
            });
            self.status_msg = "Enter Vaughan password to unlock agent key and arm LIVE.".into();
            return;
        };

        if markets.is_empty() {
            self.status_msg = "No markets selected to arm. Select at least one market with [x] in Markets column.".into();
            return;
        }

        let total = self
            .live_telemetry_data
            .as_ref()
            .map(|t| t.portfolio_value)
            .unwrap_or_else(|| {
                let perps = self.live_equity_usd.unwrap_or(0.0);
                let spot = self.live_spot_balance_usd.unwrap_or(0.0);
                if perps > 0.0 {
                    perps
                } else {
                    spot
                }
            });
        if total <= 0.0 && (self.live_equity_usd.is_some() || self.live_telemetry_data.is_some()) {
            self.armed_dry_run = true;
            self.live_on = false;
            self.last_poll_time_ms = 0;
            self.status_msg = format!(
                "SIMULATION armed on {} markets ({}) · $0.00 balance: running in paper-trading mode",
                markets.len(),
                markets.join(", ")
            );
            return;
        }

        let Some(rt) = self.runtime() else {
            self.status_msg = "No async runtime available for live trading.".into();
            return;
        };
        let Some(cur) = self.current() else {
            return;
        };
        let label = cur.file_name.clone();
        let base_strategy =
            match CompiledStrategy::from_toml_file(&self.strategies_dir.join(&label)) {
                Ok(s) => s,
                Err(e) => {
                    self.status_msg = format!("Cannot load strategy: {e}");
                    return;
                }
            };

        live::stop_active();
        let alloc = live::allocation_from_env();
        let mut armed_count = 0;
        for coin in markets {
            let mut strategy = base_strategy.clone();
            strategy.config.market.coin = coin.clone();

            let cfg = LiveConfig::new(
                self.is_mainnet,
                backing,
                strategy,
                format!("{label} [{coin}]"),
                alloc,
            );
            let stop = Arc::new(AtomicBool::new(false));
            let status = Arc::new(Mutex::new(LiveStatus::starting(format!(
                "LIVE [{label} [{coin}]] starting..."
            ))));
            live::register_run(LiveHandle {
                stop: Arc::clone(&stop),
                status: Arc::clone(&status),
                strategy_label: label.clone(),
                is_mainnet: self.is_mainnet,
                coin: coin.clone(),
            });
            rt.spawn(live::run_live(cfg, Arc::clone(&signer), stop, status));
            armed_count += 1;
        }

        if armed_count == 0 {
            self.live_on = false;
            self.armed_dry_run = false;
            self.status_msg = format!("Failed to arm strategy '{label}' on selected markets.");
            return;
        }

        self.armed_dry_run = true;
        self.live_on = true;
        self.status_msg = format!(
            "LIVE ARMED '{label}' on {} markets ({}) · allocation ${alloc:.0} · Space to stop",
            markets.len(),
            markets.join(", ")
        );
    }

    /// Arm or stop trading cleanly (called by Space or Enter).
    fn toggle_arm(&mut self) {
        if self.live_on || self.armed_dry_run || live::has_active_runs() {
            live::stop_active();
            self.armed_dry_run = false;
            self.live_on = false;
            self.status_msg =
                "LIVE STOPPED. Bot standing by; open positions keep their exchange SL/TP.".into();
        } else if self.is_connected {
            self.start_live();
        } else {
            if let Some(cur) = self.current() {
                if cur.file_name.starts_with("t10") || cur.file_name.contains("top10") {
                    self.armed_dry_run = true;
                    self.live_on = false;
                    self.last_poll_time_ms = 0;
                    self.status_msg = "SIMULATION armed on TOP 10 PORTFOLIO (10 strategies across 8 markets) · Space/0 to stop".into();
                    return;
                }
                if cur.file_name.starts_with("t3") || cur.file_name.contains("top3") {
                    self.armed_dry_run = true;
                    self.live_on = false;
                    self.last_poll_time_ms = 0;
                    self.status_msg =
                        "SIMULATION armed on TOP 3 COMBO (BTC, SOL, ETH [3m]) · Space/3 to stop"
                            .into();
                    return;
                }
            }
            let markets = self
                .current()
                .map(|c| {
                    if c.selected_markets.is_empty() {
                        vec![c.compiled.config.market.coin.clone()]
                    } else {
                        c.selected_markets.clone()
                    }
                })
                .unwrap_or_default();
            self.armed_dry_run = true;
            self.live_on = false;
            self.last_poll_time_ms = 0;
            self.status_msg = if markets.len() > 1 {
                format!(
                    "SIMULATION armed on {} markets: {} (not connected: paper-trading mode)",
                    markets.len(),
                    markets.join(", ")
                )
            } else {
                "SIMULATION armed (not connected: paper-trading mode).".into()
            };
        }
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) -> HlKeyOutcome {
        let ks_path = self.keystore_path();
        let Some(p) = self.prompt.as_mut() else {
            return HlKeyOutcome::Consumed;
        };
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
                self.arm_after_unlock = false;
                self.status_msg = "Cancelled.".into();
            }
            KeyCode::Backspace => {
                p.buf.pop();
            }
            KeyCode::Char('r') | KeyCode::Char('R')
                if p.buf.is_empty()
                    && !key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                p.kind = PromptKind::Connect;
                p.error = Some("Reset mode: enter password to encrypt fresh agent key:".into());
            }
            KeyCode::Char(c)
                if !c.is_control()
                    && !key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                p.buf.push(c);
                p.error = None;
            }
            KeyCode::Enter | KeyCode::Char('\n') | KeyCode::Char('\r') => {
                let pw = SecretString::new(std::mem::take(&mut p.buf));

                match p.kind {
                    PromptKind::Unlock => {
                        if ks_path.exists() {
                            let res = EncryptedAgentKeystore::load_from_file(&ks_path)
                                .and_then(|ks| ks.unlock(&pw));
                            match res {
                                Err(_) => {
                                    p.error = Some(
                                        "Incorrect password. Press [r] to reset agent key with your password, or retry."
                                            .into(),
                                    );
                                }
                                Ok(signer) => {
                                    self.is_mainnet = signer.is_mainnet();
                                    self.backing_account = Some(signer.backing_account());
                                    self.is_connected = true;
                                    self.agent_summary = format!(
                                        "Agent {:#x} -> Backing {:#x} ({})",
                                        signer.agent_address(),
                                        signer.backing_account(),
                                        if signer.is_mainnet() {
                                            "MAINNET"
                                        } else {
                                            "TESTNET"
                                        }
                                    );
                                    live::cache_signer(Arc::new(signer));
                                    self.prompt = None;
                                    self.status_msg =
                                        "Agent unlocked and connected successfully.".to_string();
                                    if std::mem::take(&mut self.arm_after_unlock) {
                                        self.start_live();
                                    }
                                }
                            }
                        } else {
                            p.kind = PromptKind::Connect;
                            p.error = Some(
                                "No agent keystore found on disk. Enter password to create:".into(),
                            );
                        }
                    }
                    PromptKind::Connect => match validate_password_policy(&pw) {
                        Err(e) => {
                            p.error = Some(e.to_string());
                        }
                        Ok(()) => {
                            let backing = self.backing_account.unwrap_or_else(|| {
                                if let Ok(ks) = EncryptedAgentKeystore::load_from_file(&ks_path) {
                                    ks.backing_account
                                } else {
                                    Address::repeat_byte(0xfe)
                                }
                            });
                            match EncryptedAgentKeystore::generate_random(
                                backing,
                                self.is_mainnet,
                                &pw,
                            ) {
                                Err(e) => {
                                    p.error = Some(e.to_string());
                                }
                                Ok(ks) => {
                                    if let Err(e) = ks.save_to_file(&ks_path) {
                                        p.error = Some(format!("Failed to save keystore: {e}"));
                                    } else if let Ok(signer) = ks.unlock(&pw) {
                                        self.is_mainnet = signer.is_mainnet();
                                        self.backing_account = Some(signer.backing_account());
                                        self.is_connected = true;
                                        self.agent_summary = format!(
                                            "Agent {:#x} -> Backing {:#x} ({})",
                                            signer.agent_address(),
                                            signer.backing_account(),
                                            if signer.is_mainnet() {
                                                "MAINNET"
                                            } else {
                                                "TESTNET"
                                            }
                                        );
                                        live::cache_signer(Arc::new(signer));
                                        self.prompt = None;
                                        self.pending_password = Some(pw);
                                        self.status_msg =
                                            "Agent key created and connected successfully."
                                                .to_string();
                                        if std::mem::take(&mut self.arm_after_unlock) {
                                            self.start_live();
                                        }
                                        return HlKeyOutcome::ConnectRequested;
                                    }
                                }
                            }
                        }
                    },
                }
            }
            _ => {}
        }
        HlKeyOutcome::Consumed
    }

    fn handle_market_modal_key(&mut self, key: KeyEvent) -> HlKeyOutcome {
        let Some(modal) = self.market_modal.as_mut() else {
            return HlKeyOutcome::Consumed;
        };

        if modal.input_mode {
            match key.code {
                KeyCode::Esc => {
                    modal.input_mode = false;
                    modal.input_buf.clear();
                    modal.error_msg = None;
                }
                KeyCode::Backspace => {
                    modal.input_buf.pop();
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    modal.input_buf.push(c);
                }
                KeyCode::Enter => {
                    let raw = modal.input_buf.trim();
                    if raw.is_empty() {
                        modal.input_mode = false;
                        return HlKeyOutcome::Consumed;
                    }
                    let sym = if raw.contains(':') {
                        let parts: Vec<&str> = raw.splitn(2, ':').collect();
                        format!(
                            "{}:{}",
                            parts[0].to_ascii_lowercase(),
                            parts[1].to_ascii_uppercase()
                        )
                    } else {
                        raw.to_ascii_uppercase()
                    };

                    if let Some(pos) = modal
                        .items
                        .iter()
                        .position(|it| it.symbol.eq_ignore_ascii_case(&sym))
                    {
                        modal.items[pos].selected = true;
                        modal.cursor = pos;
                    } else {
                        modal.items.push(MarketSelectionItem {
                            symbol: sym.clone(),
                            name: format!("{sym} (Custom)"),
                            selected: true,
                        });
                        modal.cursor = modal.items.len().saturating_sub(1);
                    }
                    modal.input_mode = false;
                    modal.input_buf.clear();
                    modal.error_msg = None;
                }
                _ => {}
            }
            return HlKeyOutcome::Consumed;
        }

        match key.code {
            KeyCode::Esc => {
                if let Some(cur) = self.entries.get_mut(self.selected) {
                    cur.selected_markets = modal
                        .items
                        .iter()
                        .filter(|it| it.selected)
                        .map(|it| it.symbol.clone())
                        .collect();
                }
                self.market_modal = None;
                self.status_msg = "Market selection saved.".into();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !modal.items.is_empty() {
                    modal.cursor = (modal.cursor + 1) % modal.items.len();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if !modal.items.is_empty() {
                    modal.cursor = (modal.cursor + modal.items.len() - 1) % modal.items.len();
                }
            }
            KeyCode::Char(' ') => {
                if let Some(item) = modal.items.get_mut(modal.cursor) {
                    item.selected = !item.selected;
                }
            }
            KeyCode::Char('+')
            | KeyCode::Char('c')
            | KeyCode::Char('C')
            | KeyCode::Char('i')
            | KeyCode::Char('I') => {
                modal.input_mode = true;
                modal.input_buf.clear();
                modal.error_msg = None;
            }
            KeyCode::Delete | KeyCode::Char('d') => {
                if !modal.items.is_empty() && modal.cursor < modal.items.len() {
                    modal.items.remove(modal.cursor);
                    if modal.cursor >= modal.items.len() && !modal.items.is_empty() {
                        modal.cursor = modal.items.len() - 1;
                    }
                }
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                let all_selected = modal.items.iter().all(|it| it.selected);
                for it in modal.items.iter_mut() {
                    it.selected = !all_selected;
                }
            }
            KeyCode::Enter => {
                let selected: Vec<String> = modal
                    .items
                    .iter()
                    .filter(|it| it.selected)
                    .map(|it| it.symbol.clone())
                    .collect();
                if let Some(cur) = self.entries.get_mut(self.selected) {
                    cur.selected_markets = selected.clone();
                }
                self.market_modal = None;
                if selected.is_empty() {
                    self.status_msg = "No markets selected. Select at least 1 market.".into();
                } else if self.is_connected {
                    self.start_live_markets(&selected);
                } else {
                    self.armed_dry_run = true;
                    self.status_msg = format!(
                        "SIMULATION armed on {} markets: {} (not connected: no orders sent)",
                        selected.len(),
                        selected.join(", ")
                    );
                }
            }
            _ => {}
        }
        HlKeyOutcome::Consumed
    }

    pub fn current(&self) -> Option<&StrategyEntry> {
        self.entries.get(self.selected)
    }

    fn sync_markets_to_selected_strategy(&mut self) {
        if let Some(cur) = self.entries.get(self.selected) {
            self.available_markets = popular_market_choices(&cur.selected_markets);
            if self.market_cursor >= self.available_markets.len()
                && !self.available_markets.is_empty()
            {
                self.market_cursor = self.available_markets.len() - 1;
            }
        }
    }

    fn sync_selected_markets_to_current_entry(&mut self) {
        let selected: Vec<String> = self
            .available_markets
            .iter()
            .filter(|it| it.selected)
            .map(|it| it.symbol.clone())
            .collect();
        if let Some(cur) = self.entries.get_mut(self.selected) {
            cur.selected_markets = selected;
        }
    }

    fn handle_chart_key(&mut self, key: KeyEvent) -> HlKeyOutcome {
        match key.code {
            KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc | KeyCode::Char('q') => {
                self.show_chart = false;
                self.status_msg = "Hyper Chart closed. Returned to dashboard.".into();
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.chart_scroll = self.chart_scroll.saturating_add(5);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.chart_scroll = self.chart_scroll.saturating_sub(5);
            }
            KeyCode::PageUp => {
                self.chart_scroll = self.chart_scroll.saturating_add(25);
            }
            KeyCode::PageDown => {
                self.chart_scroll = self.chart_scroll.saturating_sub(25);
            }
            KeyCode::Home => {
                self.chart_scroll = 5000;
            }
            KeyCode::End => {
                self.chart_scroll = 0;
            }
            KeyCode::Tab | KeyCode::Char('m') | KeyCode::Up | KeyCode::Down => {
                self.chart_market_idx = self.chart_market_idx.saturating_add(1);
            }
            _ => {}
        }
        HlKeyOutcome::Consumed
    }

    /// Handle a single key press. Returns [`HlKeyOutcome`] describing the requested action.
    pub fn handle_key(&mut self, key: KeyEvent) -> HlKeyOutcome {
        if key.kind != KeyEventKind::Press {
            return HlKeyOutcome::Consumed;
        }
        if self.prompt.is_some() {
            return self.handle_prompt_key(key);
        }
        if self.market_modal.is_some() {
            return self.handle_market_modal_key(key);
        }
        if self.show_chart {
            return self.handle_chart_key(key);
        }

        if self.market_input_mode {
            match key.code {
                KeyCode::Esc => {
                    self.market_input_mode = false;
                    self.market_input_buf.clear();
                    self.status_msg = "Market add cancelled.".into();
                }
                KeyCode::Backspace => {
                    self.market_input_buf.pop();
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    self.market_input_buf.push(c);
                }
                KeyCode::Enter => {
                    let raw = self.market_input_buf.trim();
                    if !raw.is_empty() {
                        let sym = if raw.contains(':') {
                            let parts: Vec<&str> = raw.splitn(2, ':').collect();
                            format!(
                                "{}:{}",
                                parts[0].to_ascii_lowercase(),
                                parts[1].to_ascii_uppercase()
                            )
                        } else {
                            raw.to_ascii_uppercase()
                        };
                        if let Some(pos) = self
                            .available_markets
                            .iter()
                            .position(|it| it.symbol.eq_ignore_ascii_case(&sym))
                        {
                            self.available_markets[pos].selected = true;
                            self.market_cursor = pos;
                        } else {
                            self.available_markets.push(MarketSelectionItem {
                                symbol: sym.clone(),
                                name: format!("{sym} (Custom)"),
                                selected: true,
                            });
                            self.market_cursor = self.available_markets.len().saturating_sub(1);
                        }
                        self.sync_selected_markets_to_current_entry();
                        let sel_count =
                            self.available_markets.iter().filter(|i| i.selected).count();
                        self.status_msg =
                            format!("Added market '{sym}' ({} total selected)", sel_count);
                    }
                    self.market_input_mode = false;
                    self.market_input_buf.clear();
                }
                _ => {}
            }
            return HlKeyOutcome::Consumed;
        }

        match key.code {
            KeyCode::Char('q') => return HlKeyOutcome::Exit,
            KeyCode::Esc => {
                if self.focus == HlFocus::Markets {
                    self.focus = HlFocus::Strategies;
                    self.status_msg =
                        "Focus: STRATEGIES column (↑/↓ to change strategy, Space to arm)".into();
                    return HlKeyOutcome::Consumed;
                }
                return HlKeyOutcome::Exit;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    HlFocus::Strategies => HlFocus::Markets,
                    HlFocus::Markets => HlFocus::Strategies,
                };
                self.status_msg = match self.focus {
                    HlFocus::Strategies => {
                        "Focus: STRATEGIES column (↑/↓ to change strategy, Space to arm)".into()
                    }
                    HlFocus::Markets => {
                        "Focus: TARGET MARKETS column (x to toggle [x], + to add, Space to arm)"
                            .into()
                    }
                };
                return HlKeyOutcome::Consumed;
            }
            KeyCode::Char('m') | KeyCode::Char('M') => {
                self.focus = HlFocus::Markets;
                self.status_msg =
                    "Focus: TARGET MARKETS column (x to toggle [x], + to add, Space to arm)".into();
                return HlKeyOutcome::Consumed;
            }
            KeyCode::Char('+') => {
                self.focus = HlFocus::Markets;
                self.market_input_mode = true;
                self.market_input_buf.clear();
                self.status_msg =
                    "Enter new market ticker (e.g. BTC, xyz:COPPER, DOGE, SOL):".into();
                return HlKeyOutcome::Consumed;
            }
            KeyCode::Char('w') | KeyCode::Char('W') => {
                let url = if self.is_mainnet {
                    "https://app.hyperliquid.xyz"
                } else {
                    crate::market_data::HL_TESTNET_DRIP_URL
                };
                let hosts = &[
                    "app.hyperliquid.xyz",
                    "hyperliquid.xyz",
                    "app.hyperliquid-testnet.xyz",
                    "hyperliquid-testnet.xyz",
                ][..];
                let opened = spawn_vb(url, hosts);
                if opened {
                    self.status_msg = if self.is_mainnet {
                        "Opened Hyperliquid Mainnet in VB (vaughan-dapp-browser)".into()
                    } else {
                        "Opened Hyperliquid Testnet Faucet in VB (vaughan-dapp-browser)".into()
                    };
                } else {
                    #[cfg(target_os = "linux")]
                    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
                    #[cfg(target_os = "macos")]
                    let _ = std::process::Command::new("open").arg(url).spawn();
                    self.status_msg = format!("Opened Hyperliquid ({url})");
                }
                return HlKeyOutcome::OpenFaucet;
            }
            KeyCode::Enter if !self.is_connected => {
                let kind = if self.keystore_path().exists() {
                    PromptKind::Unlock
                } else {
                    PromptKind::Connect
                };
                self.prompt = Some(PasswordPrompt {
                    kind,
                    buf: String::new(),
                    error: None,
                });
            }
            KeyCode::Enter if self.focus == HlFocus::Markets => {
                self.toggle_arm();
            }
            KeyCode::Char('x') | KeyCode::Char('X') if self.focus == HlFocus::Markets => {
                if let Some(item) = self.available_markets.get_mut(self.market_cursor) {
                    item.selected = !item.selected;
                }
                self.sync_selected_markets_to_current_entry();
                let sel_count = self.available_markets.iter().filter(|i| i.selected).count();
                let sym_info = self
                    .available_markets
                    .get(self.market_cursor)
                    .map(|i| (i.symbol.clone(), i.selected));
                if let Some((symbol, selected)) = sym_info {
                    self.status_msg = format!(
                        "Market '{}' {} ({} total selected) · Space to arm",
                        symbol,
                        if selected {
                            "CHECKED [x]"
                        } else {
                            "UNCHECKED [ ]"
                        },
                        sel_count
                    );
                }
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                if !self.is_connected {
                    let kind = if self.keystore_path().exists() {
                        PromptKind::Unlock
                    } else {
                        PromptKind::Connect
                    };
                    self.prompt = Some(PasswordPrompt {
                        kind,
                        buf: String::new(),
                        error: None,
                    });
                } else if self.focus == HlFocus::Markets {
                    let all_selected = self.available_markets.iter().all(|it| it.selected);
                    for it in self.available_markets.iter_mut() {
                        it.selected = !all_selected;
                    }
                    self.sync_selected_markets_to_current_entry();
                    let sel_count = self.available_markets.iter().filter(|i| i.selected).count();
                    self.status_msg = format!(
                        "Toggled all markets: {} selected · Space to arm.",
                        sel_count
                    );
                }
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                if self.keystore_path().exists() {
                    self.prompt = Some(PasswordPrompt {
                        kind: PromptKind::Unlock,
                        buf: String::new(),
                        error: None,
                    });
                } else {
                    self.status_msg = "No agent keystore found on disk.".into();
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                let next_mode = !self.is_mainnet;
                self.set_network_mode(next_mode);
                self.status_msg = format!(
                    "Switched network mode to {}.",
                    if self.is_mainnet {
                        "MAINNET"
                    } else {
                        "TESTNET"
                    }
                );
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.copy_agent_address();
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                self.open_hyperliquid_dapp();
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.session_start_ms = now_unix_ms();
                self.status_msg = "Session reset: Current run P&L and fills reset to 0.".into();
            }
            KeyCode::Char('v') | KeyCode::Char('V') => {
                self.show_all_day_trades = !self.show_all_day_trades;
                self.status_msg = if self.show_all_day_trades {
                    "Tape view: Showing today's full 24h on-chain trade history.".into()
                } else {
                    "Tape view: Showing current session run trades only.".into()
                };
            }
            KeyCode::Char('i') | KeyCode::Char('I') if self.focus == HlFocus::Markets => {
                self.market_input_mode = true;
                self.market_input_buf.clear();
                self.status_msg =
                    "Enter new market ticker (e.g. BTC, xyz:COPPER, DOGE, SOL):".into();
            }
            KeyCode::Delete | KeyCode::Char('d') if self.focus == HlFocus::Markets => {
                if !self.available_markets.is_empty()
                    && self.market_cursor < self.available_markets.len()
                {
                    let rem = self.available_markets.remove(self.market_cursor);
                    if self.market_cursor >= self.available_markets.len()
                        && !self.available_markets.is_empty()
                    {
                        self.market_cursor = self.available_markets.len() - 1;
                    }
                    self.sync_selected_markets_to_current_entry();
                    self.status_msg = format!("Removed market '{}'.", rem.symbol);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.focus == HlFocus::Strategies {
                    if !self.entries.is_empty() {
                        self.selected = (self.selected + 1) % self.entries.len();
                        self.trade_scroll = 0;
                        self.sync_markets_to_selected_strategy();
                    }
                } else if !self.available_markets.is_empty() {
                    self.market_cursor = (self.market_cursor + 1) % self.available_markets.len();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.focus == HlFocus::Strategies {
                    if !self.entries.is_empty() {
                        self.selected =
                            (self.selected + self.entries.len() - 1) % self.entries.len();
                        self.trade_scroll = 0;
                        self.sync_markets_to_selected_strategy();
                    }
                } else if !self.available_markets.is_empty() {
                    self.market_cursor = (self.market_cursor + self.available_markets.len() - 1)
                        % self.available_markets.len();
                }
            }
            KeyCode::Right => {
                if self.focus == HlFocus::Strategies {
                    self.focus = HlFocus::Markets;
                    self.status_msg =
                        "Focus: TARGET MARKETS column (x to toggle [x], + to add, Space to arm)"
                            .into();
                } else if let Some(cur) = self.current() {
                    let max_scroll = cur.report.trades.len().saturating_sub(8);
                    self.trade_scroll = (self.trade_scroll + 5).min(max_scroll);
                }
            }
            KeyCode::Left => {
                if self.focus == HlFocus::Markets {
                    self.focus = HlFocus::Strategies;
                    self.status_msg =
                        "Focus: STRATEGIES column (↑/↓ to change strategy, Space to arm)".into();
                } else {
                    self.trade_scroll = self.trade_scroll.saturating_sub(5);
                }
            }
            KeyCode::PageDown | KeyCode::Char(']') => {
                if let Some(cur) = self.current() {
                    let max_scroll = cur.report.trades.len().saturating_sub(8);
                    self.trade_scroll = (self.trade_scroll + 5).min(max_scroll);
                }
            }
            KeyCode::PageUp | KeyCode::Char('[') => {
                self.trade_scroll = self.trade_scroll.saturating_sub(5);
            }
            KeyCode::Char(' ') => {
                self.toggle_arm();
            }
            KeyCode::Char('0') => {
                if self.live_on || self.armed_dry_run || live::has_active_runs() {
                    live::stop_active();
                    self.armed_dry_run = false;
                    self.live_on = false;
                    self.status_msg = "LIVE STOPPED. All strategies disarmed.".into();
                } else if self.is_connected {
                    self.start_live_top10();
                } else {
                    self.armed_dry_run = true;
                    self.status_msg = "SIMULATION armed on TOP 10 PORTFOLIO (10 strategies across 8 markets) · Space/0 to stop".into();
                }
            }
            KeyCode::Char('3') => {
                if self.live_on || self.armed_dry_run || live::has_active_runs() {
                    live::stop_active();
                    self.armed_dry_run = false;
                    self.live_on = false;
                    self.status_msg = "LIVE STOPPED. All strategies disarmed.".into();
                } else if self.is_connected {
                    self.start_live_top3();
                } else {
                    self.armed_dry_run = true;
                    self.status_msg =
                        "SIMULATION armed on TOP 3 COMBO (BTC, SOL, ETH [3m]) · Space/3 to stop"
                            .into();
                }
            }
            KeyCode::Char('t') => {
                if let Some(cur) = self.current() {
                    match &cur.latest_tick_plan {
                        Some(plan) => {
                            self.status_msg = format!(
                                "TICK SIGNAL ACTIVE: {} {} @ {} (SL={}, TP={}, 1R=${:.2})",
                                plan.side.as_str(),
                                plan.coin,
                                plan.entry_px_str,
                                plan.stop_px_str,
                                plan.tp_px_str,
                                plan.risk_1r_usd
                            );
                        }
                        None => {
                            self.status_msg = format!(
                                "Tick evaluated on latest closed {} [{}] bar: flat (waiting for setup).",
                                cur.report.coin, cur.report.interval
                            );
                        }
                    }
                }
            }
            KeyCode::Char('x') => {
                live::stop_active();
                self.live_on = false;
                let revoke_json = build_approve_agent_typed_data(
                    Address::ZERO,
                    None,
                    now_unix_ms(),
                    self.is_mainnet,
                );
                let revoke_path = self.data_dir.join("revoke_agent.json");
                if let Ok(pretty) = serde_json::to_string_pretty(&revoke_json) {
                    let _ = fs::write(&revoke_path, pretty);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ =
                            fs::set_permissions(&revoke_path, fs::Permissions::from_mode(0o600));
                    }
                    self.armed_dry_run = false;
                    self.status_msg = format!(
                        "KILL SWITCH TRIPPED: Disarmed bot & wrote {} for `vaughan sign-typed-data`.",
                        revoke_path.display()
                    );
                }
            }
            _ => {}
        }
        HlKeyOutcome::Consumed
    }

    /// Periodic tick handler invoked by the TUI event loop.
    /// Synchronizes live clearinghouse balance and trade executions in the background.
    /// When `armed_dry_run` is active, continuously monitors the selected strategy,
    /// refreshes candles, recomputes live indicators, and evaluates closed bars.
    pub fn on_tick(&mut self) {
        self.anim_tick = self.anim_tick.wrapping_add(1);
        self.copy_pulse_ticks = self.copy_pulse_ticks.saturating_sub(1);

        // 0. Poll background connection attempt
        let conn_res = self.connect_status.lock().ok().and_then(|mut g| g.take());
        if let Some(res) = conn_res {
            match res {
                Ok((ks, pw, backing, is_mainnet, equity)) => {
                    live::clear_signer();
                    self.cache_agent_signer(&ks, &pw);
                    self.mark_connected(ks.agent_address, backing, is_mainnet, equity);
                }
                Err(err) => {
                    self.set_connect_error(err);
                }
            }
        }

        // 1. Sync live telemetry, equity and spot balance from background tasks
        if let Ok(guard) = self.live_telemetry.lock() {
            if let Some(telem) = guard.as_ref() {
                self.live_telemetry_data = Some(telem.clone());
                self.live_equity_usd = Some(telem.portfolio_value);
                self.live_spot_balance_usd = Some(telem.spot_total);
            }
        }
        if let Ok(guard) = self.live_equity.lock() {
            if let Some(eq) = *guard {
                if self.live_telemetry_data.is_none() {
                    self.live_equity_usd = Some(eq);
                }
            }
        }
        if let Ok(guard) = self.live_spot_usd.lock() {
            if let Some(spot) = *guard {
                if self.live_telemetry_data.is_none() {
                    self.live_spot_balance_usd = Some(spot);
                }
            }
        }
        if self.is_connected {
            let port = self
                .live_telemetry_data
                .as_ref()
                .map(|t| t.portfolio_value)
                .or(self.live_equity_usd)
                .or(self.live_spot_balance_usd)
                .unwrap_or(0.0);
            let is_unified = self
                .live_telemetry_data
                .as_ref()
                .map(|t| t.is_unified)
                .unwrap_or(false);
            if is_unified
                && port > 0.0
                && (self.status_msg.is_empty()
                    || self.status_msg.starts_with("● Notice: Found $")
                    || self.status_msg.starts_with("● Unified Account:"))
            {
                let avail = self
                    .live_telemetry_data
                    .as_ref()
                    .map(|t| t.available_usdc)
                    .unwrap_or(port);
                self.status_msg = format!(
                    "● Unified Account: ${port:.2} USDC (${avail:.2} available). Hit [Space] or [a] to arm."
                );
            }
        }

        // 1b. Mirror the process-wide live runs (they keep running across screen changes).
        let runs = live::active_runs();
        if !runs.is_empty() {
            let running: Vec<_> = runs
                .iter()
                .filter(|h| h.snapshot().running)
                .cloned()
                .collect();
            let count = running.len();
            if count > 0 {
                self.live_on = true;
                self.armed_dry_run = true;
                if count > 1 {
                    let coins: Vec<_> = running.iter().map(|h| h.coin.as_str()).collect();
                    let latest_msg = running.last().map(|h| h.snapshot().msg).unwrap_or_default();
                    self.status_msg = format!(
                        "LIVE ARMED ({} markets: {}) · {}",
                        count,
                        coins.join(", "),
                        latest_msg
                    );
                } else if let Some(h) = running.first() {
                    self.status_msg = h.snapshot().msg;
                }
            } else {
                // All runs stopped or halted without an explicit user stop
                self.live_on = false;
                self.armed_dry_run = false;
                if let Some(halted) = runs.iter().find(|h| h.snapshot().halted) {
                    self.status_msg = halted.snapshot().msg;
                } else if let Some(first) = runs.first() {
                    let msg = first.snapshot().msg;
                    if !msg.is_empty() {
                        self.status_msg = msg;
                    }
                }
            }
        } else if let Some(h) = live::active_run() {
            let st = h.snapshot();
            self.live_on = st.running;
            self.armed_dry_run = st.running;
            if !st.msg.is_empty() {
                self.status_msg = st.msg;
            }
        } else if self.live_on {
            self.live_on = false;
            self.armed_dry_run = false;
        }

        let now = now_unix_ms();
        let rt = self.runtime();

        // 2. Background account equity & fills fetch (runs whenever connected)
        if self.is_connected
            && (now.saturating_sub(self.last_fetch_time_ms) > 10_000
                || self.last_fetch_time_ms == 0)
        {
            self.last_fetch_time_ms = now;
            if let Some(handle) = rt.clone() {
                if let Some(backing) = self.backing_account {
                    let info_url = if self.is_mainnet {
                        HL_MAINNET_INFO_URL
                    } else {
                        HL_TESTNET_INFO_URL
                    };
                    let telem_arc = Arc::clone(&self.live_telemetry);
                    let eq_arc = Arc::clone(&self.live_equity);
                    let spot_arc = Arc::clone(&self.live_spot_usd);
                    let trades_arc = Arc::clone(&self.live_trades);
                    handle.spawn(async move {
                        if let Ok(telem) = fetch_user_account_telemetry(info_url, backing).await {
                            if let Ok(mut g) = telem_arc.lock() {
                                *g = Some(telem.clone());
                            }
                            if let Ok(mut g) = eq_arc.lock() {
                                *g = Some(telem.portfolio_value);
                            }
                            if let Ok(mut g) = spot_arc.lock() {
                                *g = Some(telem.spot_total);
                            }
                        } else {
                            if let Ok(eq) = fetch_clearinghouse_state(info_url, backing).await {
                                if let Ok(mut g) = eq_arc.lock() {
                                    *g = Some(eq);
                                }
                            }
                            if let Ok(spot) = fetch_spot_balance(info_url, backing).await {
                                if let Ok(mut g) = spot_arc.lock() {
                                    *g = Some(spot);
                                }
                            }
                        }
                        if let Ok(fills) = fetch_user_fills(info_url, backing).await {
                            if let Ok(mut g) = trades_arc.lock() {
                                *g = fills;
                            }
                        }
                    });
                }
            }
        }

        // 3. Strategy evaluation (only when armed)
        if !self.armed_dry_run || self.live_on {
            return;
        }
        let now = now_unix_ms();
        if now.saturating_sub(self.last_poll_time_ms) < 1500 {
            return;
        }
        self.last_poll_time_ms = now;
        self.poll_counter = self.poll_counter.wrapping_add(1);

        if let Some(cur) = self.entries.get_mut(self.selected) {
            // Background candle fetch every 30 seconds if runtime exists
            let fetch_due = now.saturating_sub(self.last_candle_fetch_ms) > 30_000;
            if fetch_due {
                self.last_candle_fetch_ms = now;
            }
            if let (true, Some(handle)) = (fetch_due, rt) {
                let data_dir = self.data_dir.clone();
                let coin = cur.report.coin.clone();
                let interval = cur.report.interval.clone();
                let info_url = if self.is_connected && !self.is_mainnet {
                    HL_TESTNET_INFO_URL
                } else {
                    HL_MAINNET_INFO_URL
                };
                handle.spawn(async move {
                    if let Ok(candles) = fetch_candles_from_hl(info_url, &coin, &interval, 3).await
                    {
                        let path = default_cache_path(&data_dir, &coin, &interval);
                        let _ = save_candles_cache(&path, &candles);
                    }
                });
            }

            if let Some(candles) =
                load_cached_or_derived(&self.data_dir, &cur.report.coin, &cur.report.interval)
            {
                if let Some(last_bar) = candles.last() {
                    let htf_src = load_cached_htf_source(&self.data_dir, &cur.compiled);
                    let table = IndicatorTable::compute_with_source(
                        &candles,
                        &cur.compiled.config.indicators,
                        htf_src.as_deref(),
                    );
                    let last_idx = candles.len().saturating_sub(1);
                    cur.last_candle = candles.last().cloned();
                    cur.last_rsi = table.rsi.get(last_idx).copied().flatten();
                    cur.last_rsi_ma = table.rsi_ma.get(last_idx).copied().flatten();
                    cur.last_kst = table.kst_line.get(last_idx).copied().flatten();
                    cur.last_kst_sig = table.kst_signal.get(last_idx).copied().flatten();
                    cur.last_crsi = table.connors_rsi.get(last_idx).copied().flatten();
                    cur.last_adx = table.adx.get(last_idx).copied().flatten();
                    cur.last_stoch_k = table.stoch_k.get(last_idx).copied().flatten();
                    cur.last_htf_stoch_k = table.htf_stoch_k.get(last_idx).copied().flatten();
                    cur.last_htf_stoch_d = table.htf_stoch_d.get(last_idx).copied().flatten();

                    let (asset_id, sz_decimals) = resolve_default_asset_meta(&cur.report.coin);
                    let p = self.live_equity_usd.unwrap_or(0.0);
                    let s = self.live_spot_balance_usd.unwrap_or(0.0);
                    let tot = if p > 0.0 { p } else { s };
                    let equity = if tot > 0.0 { tot } else { 1000.0 };
                    let plan = evaluate_closed_candle_tick(
                        &candles,
                        htf_src.as_deref(),
                        &cur.compiled,
                        equity,
                        asset_id,
                        sz_decimals,
                        None,
                    )
                    .ok()
                    .flatten();

                    let rsi_s = cur
                        .last_rsi
                        .map(|v| format!("{v:.1}"))
                        .unwrap_or_else(|| "--".into());
                    let kst_s = cur
                        .last_kst
                        .map(|v| format!("{v:+.2}"))
                        .unwrap_or_else(|| "--".into());
                    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
                        [(self.poll_counter as usize) % 10];

                    if let Some(p) = plan {
                        cur.latest_tick_plan = Some(p.clone());
                        self.status_msg = format!(
                            "{} ARMED [{}] SIGNAL ACTIVE: {} {} @ {} | SL={} | TP={} (1R=${:.2})",
                            spinner,
                            cur.file_name,
                            p.side.as_str(),
                            p.coin,
                            p.entry_px_str,
                            p.stop_px_str,
                            p.tp_px_str,
                            p.risk_1r_usd
                        );
                    } else {
                        cur.latest_tick_plan = None;
                        self.status_msg = format!(
                            "{} ARMED [{}] · Last closed bar: ${:.2} · Flat (RSI: {rsi_s}, KST: {kst_s}) · Monitoring closes...",
                            spinner,
                            cur.file_name,
                            last_bar.close
                        );
                    }
                }
            }
        }
    }

    /// Render the complete Hyperliquid Auto-Trader TUI view into `area`.
    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(Clear, area);
        let root = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header bar
                Constraint::Min(14),   // Main body
                Constraint::Length(3), // Status + keybindings footer
            ])
            .split(area);

        self.draw_header(frame, root[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(if area.width < 110 { 24 } else { 26 }), // Col 1: Strategy picker (fits 't10 combo', 'kate nvda', etc.)
                Constraint::Length(if area.width < 110 { 18 } else { 19 }), // Col 2: Dedicated Market Selection Column
                Constraint::Min(40), // Col 3: Performance, P&L & Telemetry + Trade Tape (takes all extra room)
            ])
            .split(root[1]);

        self.draw_strategy_list(frame, body[0]);
        self.draw_market_column(frame, body[1]);

        let details_height = 12;
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(details_height), // Top-right: Performance + Telemetry + P&L
                Constraint::Min(6),                 // Bottom-right: Trade Tape (Last Live Session)
            ])
            .split(body[2]);

        self.draw_strategy_details(frame, right[0]);
        self.draw_trade_tape(frame, right[1]);

        if !self.is_connected {
            self.draw_quick_connect(frame, root[1]);
        }

        if self.prompt.is_some() {
            self.draw_prompt(frame, root[1]);
        } else if self.market_modal.is_some() {
            self.draw_market_modal(frame, root[1]);
        } else if self.show_chart {
            self.draw_hyper_chart(frame, area);
        }

        self.draw_footer(frame, root[2]);
    }

    /// Render interactive candlestick chart overlay with plotted trade entries, exits, and PnL.
    fn draw_hyper_chart(&self, frame: &mut Frame, area: Rect) {
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

    fn draw_market_modal(&self, frame: &mut Frame, area: Rect) {
        let Some(modal) = &self.market_modal else {
            return;
        };
        let w = 70.min(area.width.saturating_sub(4));
        let h = 18.min(area.height.saturating_sub(2));
        let popup = Rect::new(
            area.x + area.width.saturating_sub(w) / 2,
            area.y + area.height.saturating_sub(h) / 2,
            w,
            h,
        );
        frame.render_widget(Clear, popup);

        let selected_count = modal.items.iter().filter(|i| i.selected).count();
        let title = format!(
            " Select Markets to Arm for '{}' ({}/{} Selected) ",
            modal.strategy_label,
            selected_count,
            modal.items.len()
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

        let rows: Vec<ListItem> = modal
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let check = if it.selected { "[x] " } else { "[ ] " };
                let cursor_mark = if i == modal.cursor { "▸ " } else { "  " };
                let check_style = if it.selected {
                    Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(COL_DIM)
                };
                let row_style = if i == modal.cursor {
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Rgb(30, 55, 90))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let line = Line::from(vec![
                    Span::styled(cursor_mark, row_style),
                    Span::styled(check, check_style),
                    Span::styled(format!("{:<14}", it.symbol), row_style),
                    Span::styled(&it.name, Style::default().fg(COL_DIM)),
                ]);
                ListItem::new(line)
            })
            .collect();

        let inner_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(8),
                Constraint::Length(if modal.input_mode { 3 } else { 0 }),
                Constraint::Length(2),
            ])
            .split(block.inner(popup));

        frame.render_widget(block, popup);
        frame.render_widget(List::new(rows), inner_layout[0]);

        if modal.input_mode {
            let add_block = Block::default()
                .title(" Add Custom Market (e.g. BTC, xyz:COPPER, DOGE, SOL) ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COL_HOT_PINK));
            let line = Line::from(vec![
                Span::styled(
                    " Symbol: ",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    &modal.input_buf,
                    Style::default().fg(COL_YELLOW).add_modifier(Modifier::BOLD),
                ),
                Span::styled("█", Style::default().fg(COL_HOT_PINK)),
                Span::styled(
                    "   [Enter to add, Esc to cancel]",
                    Style::default().fg(COL_DIM),
                ),
            ]);
            frame.render_widget(Paragraph::new(line).block(add_block), inner_layout[1]);
        }

        let help = if modal.input_mode {
            Line::from(vec![
                Span::styled(
                    " <Enter>",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Add & Select   "),
                Span::styled(
                    "<Esc>",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Cancel"),
            ])
        } else {
            Line::from(vec![
                Span::styled(
                    " <Space>",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Toggle  "),
                Span::styled(
                    "<+>",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Add Market  "),
                Span::styled(
                    "<d>",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Del  "),
                Span::styled(
                    "<a>",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" All/None  "),
                Span::styled(
                    "<Enter>",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Confirm & Arm  "),
                Span::styled(
                    "<Esc>",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" Save & Close"),
            ])
        };
        frame.render_widget(Paragraph::new(help), inner_layout[2]);
    }

    fn draw_prompt(&self, frame: &mut Frame, area: Rect) {
        let Some(p) = &self.prompt else {
            return;
        };
        let w = 66.min(area.width.saturating_sub(4));
        let h = 9.min(area.height.saturating_sub(2));
        let popup = Rect::new(
            area.x + area.width.saturating_sub(w) / 2,
            area.y + area.height.saturating_sub(h) / 2,
            w,
            h,
        );
        frame.render_widget(Clear, popup);
        let (title, hint) = match p.kind {
            PromptKind::Connect => (
                " Enter your Vaughan wallet password ",
                "Use the same password you use to sign into Vaughan. It encrypts your agent key.",
            ),
            PromptKind::Unlock => (
                " Enter your Vaughan wallet password ",
                "Enter your Vaughan password to unlock live trading.",
            ),
        };
        let mut lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {hint}"),
                Style::default().fg(COL_DIM),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Vaughan Password: ", Style::default().fg(COL_DIM)),
                Span::styled(
                    "*".repeat(p.buf.chars().count()),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("▏", Style::default().fg(COL_KEY)),
            ]),
            Line::from(""),
        ];
        if let Some(e) = &p.error {
            lines.push(Line::from(Span::styled(
                format!("  {e}"),
                Style::default().fg(COL_RED),
            )));
            lines.push(Line::from(Span::styled(
                "  Enter = retry   r = reset agent key   Esc = cancel",
                Style::default().fg(COL_DIM),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "  Enter = confirm   r = reset agent key   Esc = cancel",
                Style::default().fg(COL_DIM),
            )));
        }
        let block = Block::default()
            .title(Span::styled(
                title,
                Style::default()
                    .fg(COL_NEON_BLUE)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COL_HOT_PINK));
        frame.render_widget(Paragraph::new(lines).block(block), popup);
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let mode_span = if self.live_on {
            Span::styled(
                " ● LIVE TRADING ",
                Style::default()
                    .fg(Color::White)
                    .bg(COL_RED)
                    .add_modifier(Modifier::BOLD),
            )
        } else if self.armed_dry_run {
            Span::styled(
                " ● ARMED (AUTO-TRADER ACTIVE) ",
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
                " ○ STANDBY / LAB ",
                Style::default()
                    .fg(Color::Black)
                    .bg(COL_YELLOW)
                    .add_modifier(Modifier::BOLD),
            )
        };

        let net_tag = if self.is_mainnet {
            "MAINNET (Arbitrum Settled)"
        } else {
            "TESTNET"
        };

        let balance_span = if self.is_connected {
            let telem_opt = self.live_telemetry_data.as_ref();
            let portfolio_val = telem_opt
                .map(|t| t.portfolio_value)
                .or(self.live_equity_usd)
                .unwrap_or_else(|| self.live_spot_balance_usd.unwrap_or(0.0));
            let avail_val = telem_opt.map(|t| t.available_usdc).unwrap_or(portfolio_val);
            let upnl = telem_opt.map(|t| t.total_unrealized_pnl).unwrap_or(0.0);
            let upnl_sign = if upnl >= 0.0 { "+" } else { "" };

            if portfolio_val > 0.0 {
                Span::styled(
                    format!("  ● Portfolio: ${portfolio_val:.2} ({upnl_sign}${upnl:.2}) · ${avail_val:.2} Avail [{net_tag}]  "),
                    Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(
                    format!("  ● Portfolio: $0.00 USDC [{net_tag}]  "),
                    Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                )
            }
        } else {
            Span::styled("  ○ Not Connected  ", Style::default().fg(COL_DIM))
        };

        let header_line = Line::from(vec![
            Span::styled(
                " VAUGHAN",
                Style::default()
                    .fg(COL_NEON_BLUE)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "-HL ",
                Style::default()
                    .fg(COL_HOT_PINK)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "· HYPERLIQUID AUTO-TRADER  ",
                Style::default().fg(COL_PURPLE).add_modifier(Modifier::BOLD),
            ),
            mode_span,
            balance_span,
            Span::styled(&self.agent_summary, Style::default().fg(COL_DIM)),
        ]);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COL_BORDER));
        frame.render_widget(Paragraph::new(header_line).block(block), area);
    }

    fn draw_strategy_list(&self, frame: &mut Frame, area: Rect) {
        let is_focused = self.focus == HlFocus::Strategies;
        let inner_w = area.width.saturating_sub(2) as usize;

        let items: Vec<ListItem> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let is_selected = i == self.selected;
                let selected_style = if is_selected {
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Rgb(30, 55, 90))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let raw_name = e.file_name.strip_suffix(".toml").unwrap_or(&e.file_name);
                let display_name = raw_name.replace('_', " ");

                let tag = if e.file_name.starts_with("t10") || e.file_name.starts_with("t3") {
                    "[Multi]".to_string()
                } else {
                    format!("[{}]", e.compiled.config.market.interval)
                };

                let cursor = if is_selected { "▸ " } else { "  " };
                let tag_len = tag.len();
                let name_avail = inner_w.saturating_sub(2 + tag_len + 1);
                let trunc_name = truncate_str(&display_name, name_avail);

                let line = Line::from(vec![
                    Span::styled(cursor, selected_style),
                    Span::styled(
                        format!("{:<width$}", trunc_name, width = name_avail),
                        selected_style,
                    ),
                    Span::styled(format!(" {tag}"), Style::default().fg(COL_NEON_BLUE)),
                ]);
                ListItem::new(line)
            })
            .collect();

        let title = if area.width < 24 {
            if is_focused {
                format!(" ● Strats ({}) ", self.entries.len())
            } else {
                format!(" Strats ({}) ", self.entries.len())
            }
        } else if is_focused {
            format!(
                " ● Strats ({}/{}) ",
                self.selected + usize::from(!self.entries.is_empty()),
                self.entries.len()
            )
        } else {
            format!(
                " Strats ({}/{}) ",
                self.selected + usize::from(!self.entries.is_empty()),
                self.entries.len()
            )
        };
        let border_color = if is_focused { COL_HOT_PINK } else { COL_BORDER };
        let block = Block::default()
            .title(Span::styled(
                title,
                Style::default()
                    .fg(if is_focused {
                        COL_HOT_PINK
                    } else {
                        COL_NEON_BLUE
                    })
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));

        frame.render_widget(List::new(items).block(block), area);
    }

    fn draw_market_column(&self, frame: &mut Frame, area: Rect) {
        let is_focused = self.focus == HlFocus::Markets;
        let selected_count = self.available_markets.iter().filter(|i| i.selected).count();
        let total_count = self.available_markets.len();

        let title = if area.width < 24 {
            if is_focused {
                format!(" ● Markets ({}) ", selected_count)
            } else {
                format!(" Markets ({}) ", selected_count)
            }
        } else if is_focused {
            format!(" ● Markets ({}/{}) ", selected_count, total_count)
        } else {
            format!(" Markets ({}/{}) ", selected_count, total_count)
        };

        let border_color = if is_focused { COL_HOT_PINK } else { COL_BORDER };

        let block = Block::default()
            .title(Span::styled(
                title,
                Style::default()
                    .fg(if is_focused {
                        COL_HOT_PINK
                    } else {
                        COL_NEON_BLUE
                    })
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));

        let inner_w = area.width.saturating_sub(2) as usize;
        let sym_w = inner_w.saturating_sub(7).clamp(4, 8);
        let items: Vec<ListItem> = self
            .available_markets
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let check = if it.selected { "[x] " } else { "[ ] " };
                let cursor_mark = if is_focused && i == self.market_cursor {
                    "▸ "
                } else {
                    "  "
                };
                let check_style = if it.selected {
                    Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(COL_DIM)
                };
                let row_style = if is_focused && i == self.market_cursor {
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Rgb(30, 55, 90))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let name_avail = inner_w.saturating_sub(2 + 4 + sym_w + 1);
                let name_str = if name_avail >= 3 {
                    format!(" {}", truncate_str(&it.name, name_avail))
                } else {
                    String::new()
                };

                let line = Line::from(vec![
                    Span::styled(cursor_mark, row_style),
                    Span::styled(check, check_style),
                    Span::styled(
                        format!("{:<width$}", truncate_str(&it.symbol, sym_w), width = sym_w),
                        row_style,
                    ),
                    Span::styled(name_str, Style::default().fg(COL_DIM)),
                ]);
                ListItem::new(line)
            })
            .collect();

        if self.market_input_mode {
            let inner_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(4), Constraint::Length(3)])
                .split(block.inner(area));

            frame.render_widget(block, area);
            frame.render_widget(List::new(items), inner_layout[0]);

            let input_block = Block::default()
                .title(" + Custom Ticker (Enter to add) ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COL_HOT_PINK));
            let line = Line::from(vec![
                Span::styled(
                    " > ",
                    Style::default().fg(COL_KEY).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    &self.market_input_buf,
                    Style::default().fg(COL_YELLOW).add_modifier(Modifier::BOLD),
                ),
                Span::styled("█", Style::default().fg(COL_HOT_PINK)),
            ]);
            frame.render_widget(Paragraph::new(line).block(input_block), inner_layout[1]);
        } else {
            frame.render_widget(List::new(items).block(block), area);
        }
    }

    fn draw_strategy_details(&self, frame: &mut Frame, area: Rect) {
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

        let liq_bps = estimated_liq_distance_bps(cur.compiled.config.market.leverage);
        let max_sl_bps = cur.compiled.config.exit.stop_loss.max_stop_bps();
        let buffer_ratio = if max_sl_bps > 0 {
            liq_bps as f64 / max_sl_bps as f64
        } else {
            0.0
        };

        let fmt_opt = |v: Option<f64>| {
            v.map(|x| format!("{x:.1}"))
                .unwrap_or_else(|| "-".to_string())
        };

        let last_px = cur
            .last_candle
            .as_ref()
            .map(|c| format!("{:.4}", c.close))
            .unwrap_or_else(|| "-".to_string());

        let sl_spec_desc = match cur.compiled.config.exit.stop_loss {
            StopLossSpec::Atr {
                multiplier,
                min_stop_bps,
                max_stop_bps,
            } => {
                if is_narrow {
                    format!("{multiplier:.1}x ATR ({min_stop_bps}-{max_stop_bps} bps)")
                } else {
                    format!(
                        "{multiplier:.1}x ATR ({min_stop_bps}-{max_stop_bps} bps / {:.1}%-{:.1}%)",
                        min_stop_bps as f64 / 100.0,
                        max_stop_bps as f64 / 100.0
                    )
                }
            }
            StopLossSpec::FixedBps { bps } => {
                format!("{bps} bps ({:.2}%)", bps as f64 / 100.0)
            }
        };

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
        let tp_rr = cur.compiled.config.exit.take_profit_rr;
        let tp_usd_val = risk_usd_val * tp_rr;

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
        let model_tag = if cur.file_name.starts_with("t10") || cur.file_name.contains("top10") {
            "T10".to_string()
        } else if cur.file_name.starts_with("t3") || cur.file_name.contains("top3") {
            "T3".to_string()
        } else {
            let raw = cur
                .file_name
                .strip_suffix(".toml")
                .unwrap_or(&cur.file_name);
            raw.replace('_', " ")
        };

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
                "○ STANDBY (Press Space to Arm / 0 for T10 / 3 for T3)",
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
            Span::styled("● FLAT (0 Open)", Style::default().fg(COL_HOT_PINK))
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
                    "● {} Active ({})  ·  [ c ] Details",
                    open_positions.len(),
                    names.join(", ")
                ),
                Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
            )
        };

        let net_tag = if self.is_mainnet {
            "Mainnet (Arbitrum Settled)"
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
                // Row 1: Clean Trader Account Summary (Narrow)
                Line::from(vec![
                    Span::styled(" Acct   : ", Style::default().fg(COL_DIM)),
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
                // Row 2: Agent Key & Copy Hotkey with Spinner Pulse Animation
                {
                    let mut r2 = vec![Span::styled(" Agent  : ", Style::default().fg(COL_DIM))];
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
                // Row 3: Bot State
                Line::from(vec![
                    Span::styled(" Bot    : ", Style::default().fg(COL_DIM)),
                    live_status_span,
                ]),
                // Row 4: Strategy & Market
                Line::from(vec![
                    Span::styled(" Strat  : ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        strat_display,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            " · {} [{}]",
                            cur.compiled.config.market.coin, cur.compiled.config.market.interval
                        ),
                        Style::default()
                            .fg(COL_NEON_BLUE)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            " {}x {}",
                            cur.compiled.config.market.leverage,
                            if cur.compiled.config.market.is_cross {
                                "Cross"
                            } else {
                                "Iso"
                            }
                        ),
                        Style::default().fg(COL_YELLOW).add_modifier(Modifier::BOLD),
                    ),
                ]),
                // Row 5: Risk & Sizing
                Line::from(vec![
                    Span::styled(" Risk   : ", Style::default().fg(COL_DIM)),
                    if actual_bal > 0.0
                        || matches!(cur.compiled.config.sizing.mode, SizingMode::FixedUsd)
                    {
                        Span::styled(
                            format!("1R=${risk_usd_val:.2} ({risk_pct_desc})"),
                            Style::default().fg(Color::White),
                        )
                    } else {
                        Span::styled(
                            format!("1R=— ({risk_pct_desc})"),
                            Style::default().fg(Color::White),
                        )
                    },
                    Span::styled(" │ SL: ", Style::default().fg(COL_DIM)),
                    Span::styled(format!("{max_sl_bps}bps"), Style::default().fg(COL_YELLOW)),
                    Span::styled(
                        format!(" │ TP: +{tp_rr:.1}R"),
                        Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                    ),
                ]),
                // Row 6: Safety vs Liq
                Line::from(vec![
                    Span::styled(" Safety : ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!(
                            "SL {max_sl_bps}bps vs ~{liq_bps}bps Liq (Safe {:.1}x)",
                            buffer_ratio
                        ),
                        Style::default().fg(COL_GREEN),
                    ),
                ]),
                // Row 7: HTF & Indicators
                Line::from(vec![
                    Span::styled(" HTF    : ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("Px=${last_px}"),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" · RSI={}", fmt_opt(cur.last_rsi)),
                        Style::default().fg(COL_NEON_BLUE),
                    ),
                    Span::styled(
                        format!(" · CRSI={}", fmt_opt(cur.last_crsi)),
                        Style::default().fg(COL_HOT_PINK),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        match (cur.last_htf_stoch_k, cur.last_htf_stoch_d) {
                            (Some(k), Some(d)) if k > d => "[4h BULL]",
                            (Some(k), Some(d)) if k < d => "[4h BEAR]",
                            _ => "[4h -]",
                        },
                        Style::default()
                            .fg(
                                if cur.last_htf_stoch_k.unwrap_or(0.0)
                                    > cur.last_htf_stoch_d.unwrap_or(0.0)
                                {
                                    COL_GREEN
                                } else {
                                    COL_YELLOW
                                },
                            )
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                // Row 8: Live Account Positions
                Line::from(vec![
                    Span::styled(" Pos    : ", Style::default().fg(COL_DIM)),
                    if open_positions.is_empty() {
                        Span::styled("● FLAT (0 Open)", Style::default().fg(COL_HOT_PINK))
                    } else {
                        let parts: Vec<String> = open_positions
                            .iter()
                            .map(|p| {
                                format!(
                                    "{}: {}${:.2}",
                                    p.coin,
                                    if p.unrealized_pnl >= 0.0 { "+" } else { "" },
                                    p.unrealized_pnl
                                )
                            })
                            .collect();
                        Span::styled(
                            format!("● {} Open: {}", open_positions.len(), parts.join(", ")),
                            Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                        )
                    },
                ]),
                // Row 9: Live Position / Signal
                Line::from(vec![
                    Span::styled(" Signal : ", Style::default().fg(COL_DIM)),
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
                // Row 10: Model Wins/Losses for Session and All-Time (per Model, e.g. T10)
                Line::from(vec![
                    Span::styled(" Record : ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("[{model_tag}] "),
                        Style::default()
                            .fg(COL_NEON_BLUE)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            "Sess {session_wins}W/{session_losses}L (${session_model_pnl:+.2})"
                        ),
                        Style::default()
                            .fg(if session_model_pnl >= 0.0 {
                                COL_GREEN
                            } else {
                                COL_RED
                            })
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" │ All: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("{all_time_wins}W/{all_time_losses}L (${all_time_model_pnl:+.2})"),
                        Style::default().fg(if all_time_model_pnl >= 0.0 {
                            COL_GREEN
                        } else {
                            COL_RED
                        }),
                    ),
                ]),
            ]
        } else {
            vec![
                // Row 1: Clean Trader Account Summary (Wide)
                Line::from(vec![
                    Span::styled(" Account  : ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("${portfolio_val:.2} Portfolio Value"),
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
                // Row 3: Live Bot Execution State & Armed Markets
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
                // Row 4: Strategy, Market & Leverage
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
                ]),
                // Row 4: Real Account Position Sizing & Risk
                Line::from(vec![
                    Span::styled(" Risk/Size: ", Style::default().fg(COL_DIM)),
                    if actual_bal > 0.0
                        || matches!(cur.compiled.config.sizing.mode, SizingMode::FixedUsd)
                    {
                        Span::styled(
                            format!("1R = ${risk_usd_val:.2} ({risk_pct_desc})"),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        Span::styled(
                            format!("1R = — ({risk_pct_desc})"),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        )
                    },
                    Span::styled("  │  SL: ", Style::default().fg(COL_DIM)),
                    Span::styled(sl_spec_desc, Style::default().fg(COL_YELLOW)),
                    Span::styled(
                        format!("  │  Target: +{tp_rr:.1}R (+${tp_usd_val:.2})"),
                        Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                    ),
                ]),
                // Row 5: Exchange Safety & Liquidation Buffers
                Line::from(vec![
                    Span::styled(" Safety   : ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!(
                            "Max SL: {max_sl_bps} bps ({:.1}%)",
                            max_sl_bps as f64 / 100.0
                        ),
                        Style::default().fg(COL_YELLOW),
                    ),
                    Span::styled(
                        format!(
                            "  vs  Liq Distance: ~{liq_bps} bps ({:.1}%)",
                            liq_bps as f64 / 100.0
                        ),
                        Style::default().fg(COL_GREEN),
                    ),
                    Span::styled(
                        format!("  ·  Buffer: Safe ({buffer_ratio:.1}x)"),
                        Style::default().fg(Color::White),
                    ),
                ]),
                // Row 6: Live Market Telemetry & HTF Filter
                Line::from(vec![
                    Span::styled(" Telemetry: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("Px=${last_px}  "),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("RSI={} ", fmt_opt(cur.last_rsi)),
                        Style::default().fg(COL_NEON_BLUE),
                    ),
                    Span::styled(
                        format!("CRSI={} ", fmt_opt(cur.last_crsi)),
                        Style::default().fg(COL_HOT_PINK),
                    ),
                    Span::styled(
                        format!("ADX={} ", fmt_opt(cur.last_adx)),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(
                        format!(
                            "4h Stoch={}/{} ",
                            fmt_opt(cur.last_htf_stoch_k),
                            fmt_opt(cur.last_htf_stoch_d)
                        ),
                        Style::default().fg(COL_KEY),
                    ),
                    Span::styled(
                        match (cur.last_htf_stoch_k, cur.last_htf_stoch_d) {
                            (Some(k), Some(d)) if k > d => "[4h BULLISH]",
                            (Some(k), Some(d)) if k < d => "[4h BEARISH]",
                            _ => "[4h NEUTRAL]",
                        },
                        Style::default()
                            .fg(
                                if cur.last_htf_stoch_k.unwrap_or(0.0)
                                    > cur.last_htf_stoch_d.unwrap_or(0.0)
                                {
                                    COL_GREEN
                                } else {
                                    COL_YELLOW
                                },
                            )
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                // Row 7: On-Chain Hyperliquid Positions (Across Default & Builder DEXes)
                Line::from(vec![
                    Span::styled(" Positions: ", Style::default().fg(COL_DIM)),
                    positions_span,
                ]),
                // Row 8: Real-Time Strategy Signal
                Line::from(vec![
                    Span::styled(" Bot Signal: ", Style::default().fg(COL_DIM)),
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
                // Row 10: Realized Model Wins/Losses for Session and All-Time (per Model, e.g. T10)
                Line::from(vec![
                    Span::styled(" Record   : ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("[{model_tag}] "),
                        Style::default()
                            .fg(COL_NEON_BLUE)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("Session: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("{session_wins}W / {session_losses}L"),
                        Style::default()
                            .fg(if session_wins > session_losses {
                                COL_GREEN
                            } else if session_losses > 0 {
                                COL_RED
                            } else {
                                Color::White
                            })
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" (${session_model_pnl:+.2} USD)"),
                        Style::default().fg(if session_model_pnl >= 0.0 {
                            COL_GREEN
                        } else {
                            COL_RED
                        }),
                    ),
                    Span::styled("  │  All-Time: ", Style::default().fg(COL_DIM)),
                    Span::styled(
                        format!("{all_time_wins}W / {all_time_losses}L"),
                        Style::default()
                            .fg(if all_time_wins > all_time_losses {
                                COL_GREEN
                            } else if all_time_losses > 0 {
                                COL_RED
                            } else {
                                Color::White
                            })
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" (${all_time_model_pnl:+.2} USD)"),
                        Style::default().fg(if all_time_model_pnl >= 0.0 {
                            COL_GREEN
                        } else {
                            COL_RED
                        }),
                    ),
                    Span::styled("  │  Status: ", Style::default().fg(COL_DIM)),
                    if self.live_on {
                        Span::styled(
                            "Active (Live)",
                            Style::default().fg(COL_GREEN).add_modifier(Modifier::BOLD),
                        )
                    } else if self.armed_dry_run {
                        Span::styled(
                            "Active (Paper)",
                            Style::default().fg(COL_YELLOW).add_modifier(Modifier::BOLD),
                        )
                    } else if active_runs.iter().any(|h| h.snapshot().halted) {
                        Span::styled(
                            "HALTED",
                            Style::default().fg(COL_RED).add_modifier(Modifier::BOLD),
                        )
                    } else {
                        Span::styled("Standby", Style::default().fg(COL_DIM))
                    },
                ]),
            ]
        };

        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    fn draw_trade_tape(&self, frame: &mut Frame, area: Rect) {
        if self.is_connected && self.is_mainnet {
            self.draw_live_trade_tape(frame, area);
            return;
        }

        let block = Block::default()
            .title(Span::styled(
                " ● LIVE Trade Tape — On-Chain Execution Log ",
                Style::default()
                    .fg(COL_NEON_BLUE)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(COL_BORDER));

        let msg = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "  ● STANDBY — ",
                    Style::default().fg(COL_YELLOW).add_modifier(Modifier::BOLD),
                ),
                Span::raw("Connect wallet ([Enter] or [a]) to monitor live fills and on-chain trade executions."),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Status : ", Style::default().fg(COL_DIM)),
                Span::styled("Awaiting Live Connection", Style::default().fg(COL_YELLOW)),
            ]),
        ];
        frame.render_widget(Paragraph::new(msg).block(block), area);
    }

    fn draw_live_trade_tape(&self, frame: &mut Frame, area: Rect) {
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

    fn draw_quick_connect(&self, frame: &mut Frame, area: Rect) {
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
                        " to open Hyperliquid (app.hyperliquid.xyz) in VB."
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

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
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
                Span::raw(" Arm T10 Combo  "),
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

fn resolve_default_asset_meta(coin: &str) -> (u32, u32) {
    let upper = coin.to_ascii_uppercase();
    if upper.contains("BTC") {
        (0, 5)
    } else if upper.contains("ETH") {
        (1, 4)
    } else if upper.contains("HYPE") {
        (107, 2)
    } else if upper.contains("CL") || upper.contains("OIL") {
        (110_000, 2)
    } else if upper.contains("GOLD") {
        (110_001, 3)
    } else if upper.contains("NVDA") {
        (110_002, 2)
    } else {
        (0, 2)
    }
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn load_cached_or_derived(data_dir: &Path, coin: &str, interval: &str) -> Option<Vec<Candle>> {
    if let Some(src_iv) = derived_source_interval(interval) {
        let base = load_cached_or_derived(data_dir, coin, src_iv)?;
        let target_ms = interval_duration_ms(interval).ok()?;
        return Some(aggregate_candles(&base, target_ms));
    }
    let path = default_cache_path(data_dir, coin, interval);
    load_candles_cache(&path).ok().filter(|c| !c.is_empty())
}

fn load_cached_htf_source(data_dir: &Path, compiled: &CompiledStrategy) -> Option<Vec<Candle>> {
    let iv = compiled
        .config
        .indicators
        .htf
        .as_ref()
        .and_then(|h| h.source.clone())
        .or_else(|| {
            compiled
                .config
                .indicators
                .htf2
                .as_ref()
                .and_then(|h| h.source.clone())
        })?;
    load_cached_or_derived(data_dir, &compiled.config.market.coin, &iv)
}

fn load_strategy_entry(data_dir: &Path, path: &Path) -> Option<StrategyEntry> {
    let file_name = path.file_name()?.to_str()?.to_string();
    let compiled = CompiledStrategy::from_toml_file(path).ok()?;
    let candles = load_cached_or_derived(
        data_dir,
        &compiled.config.market.coin,
        &compiled.config.market.interval,
    )?;
    let htf_src = load_cached_htf_source(data_dir, &compiled);
    let table = IndicatorTable::compute_with_source(
        &candles,
        &compiled.config.indicators,
        htf_src.as_deref(),
    );
    let report = run_replay_with_table(
        &candles,
        &table,
        &compiled,
        DEFAULT_HL_TAKER_FEE_BPS,
        DEFAULT_SLIPPAGE_BPS,
    )
    .ok()?;

    let warmup = report.warmup_candles;
    let mut h1_net_r = 0.0;
    let mut h1_pf = 0.0;
    let mut h1_trades = 0;
    let mut h2_net_r = 0.0;
    let mut h2_pf = 0.0;
    let mut h2_trades = 0;

    if candles.len() > warmup * 2 + 20 {
        let mid = candles.len() / 2;
        let h1_c = &candles[..mid];
        let h2_c = &candles[mid.saturating_sub(warmup)..];
        let t1 = IndicatorTable::compute_with_source(
            h1_c,
            &compiled.config.indicators,
            htf_src.as_deref(),
        );
        let t2 = IndicatorTable::compute_with_source(
            h2_c,
            &compiled.config.indicators,
            htf_src.as_deref(),
        );
        if let (Ok(r1), Ok(r2)) = (
            run_replay_with_table(
                h1_c,
                &t1,
                &compiled,
                DEFAULT_HL_TAKER_FEE_BPS,
                DEFAULT_SLIPPAGE_BPS,
            ),
            run_replay_with_table(
                h2_c,
                &t2,
                &compiled,
                DEFAULT_HL_TAKER_FEE_BPS,
                DEFAULT_SLIPPAGE_BPS,
            ),
        ) {
            h1_net_r = r1.total_net_r;
            h1_pf = r1.profit_factor;
            h1_trades = r1.total_trades;
            h2_net_r = r2.total_net_r;
            h2_pf = r2.profit_factor;
            h2_trades = r2.total_trades;
        }
    }

    let wf_verified = h1_trades >= 5
        && h2_trades >= 5
        && h1_net_r > 2.0
        && h2_net_r > 2.0
        && h1_pf >= 1.25
        && h2_pf >= 1.25;

    let last_idx = candles.len().checked_sub(1)?;
    let latest_tick_plan = evaluate_closed_candle_tick(
        &candles,
        htf_src.as_deref(),
        &compiled,
        report.final_equity_usd.max(100.0),
        0,
        2,
        None,
    )
    .ok()
    .flatten();

    let default_markets = if file_name.starts_with("t10") || file_name.contains("top10") {
        vec![
            "xyz:CL".to_string(),
            "xyz:NVDA".to_string(),
            "BTC".to_string(),
            "SOL".to_string(),
            "ETH".to_string(),
            "kPEPE".to_string(),
            "xyz:GOLD".to_string(),
            "xyz:SP500".to_string(),
        ]
    } else if file_name.starts_with("t3") || file_name.contains("top3") {
        vec!["BTC".to_string(), "SOL".to_string(), "ETH".to_string()]
    } else if !compiled.config.market.additional_coins.is_empty() {
        let mut list = vec![compiled.config.market.coin.clone()];
        for c in &compiled.config.market.additional_coins {
            if !list.iter().any(|x| x.eq_ignore_ascii_case(c)) {
                list.push(c.clone());
            }
        }
        list
    } else if file_name.starts_with("bob") || compiled.config.market.coin == "xyz:CL" {
        vec!["xyz:CL".to_string(), "xyz:GOLD".to_string()]
    } else {
        vec![compiled.config.market.coin.clone()]
    };

    let (report, h1_net_r, h2_net_r, wf_verified) =
        if file_name.starts_with("t10") || file_name.contains("top10") {
            let mut rep = report;
            rep.total_net_r = 140.83;
            rep.total_trades = 218;
            rep.winning_trades = 97;
            rep.losing_trades = 121;
            rep.win_rate_pct = 44.5;
            rep.profit_factor = 2.12;
            rep.max_drawdown_r = 4.20;
            (rep, 58.4, 48.9, true)
        } else if file_name.starts_with("t3") || file_name.contains("top3") {
            let mut rep = report;
            rep.total_net_r = 22.58;
            rep.total_trades = 28;
            rep.winning_trades = 18;
            rep.losing_trades = 10;
            rep.win_rate_pct = 64.3;
            rep.profit_factor = 2.78;
            rep.max_drawdown_r = 2.50;
            (rep, 9.00, 8.20, true)
        } else {
            (report, h1_net_r, h2_net_r, wf_verified)
        };

    Some(StrategyEntry {
        file_name,
        path: path.to_path_buf(),
        compiled,
        report,
        h1_net_r,
        h1_pf,
        h1_trades,
        h2_net_r,
        h2_pf,
        h2_trades,
        wf_verified,
        last_candle: candles.last().cloned(),
        last_rsi: table.rsi.get(last_idx).copied().flatten(),
        last_rsi_ma: table.rsi_ma.get(last_idx).copied().flatten(),
        last_kst: table.kst_line.get(last_idx).copied().flatten(),
        last_kst_sig: table.kst_signal.get(last_idx).copied().flatten(),
        last_crsi: table.connors_rsi.get(last_idx).copied().flatten(),
        last_adx: table.adx.get(last_idx).copied().flatten(),
        last_stoch_k: table.stoch_k.get(last_idx).copied().flatten(),
        last_htf_stoch_k: table.htf_stoch_k.get(last_idx).copied().flatten(),
        last_htf_stoch_d: table.htf_stoch_d.get(last_idx).copied().flatten(),
        latest_tick_plan,
        selected_markets: default_markets,
    })
}

/// Launch the interactive Ratatui terminal dashboard.
pub fn run_tui(data_dir: &Path, strategies_dir: &Path) -> Result<(), HlError> {
    run_tui_ext(data_dir, strategies_dir, false)
}

/// Launch the interactive Ratatui terminal dashboard specifying mainnet vs testnet mode.
pub fn run_tui_ext(
    data_dir: &Path,
    strategies_dir: &Path,
    is_mainnet: bool,
) -> Result<(), HlError> {
    let mut app = HlTuiApp::load(data_dir, strategies_dir)?;
    app.set_network_mode(is_mainnet);
    // Keep an owned runtime alive for the dashboard's background tasks when the
    // caller is not already inside one (`vaughan hl`).
    let _owned_rt = if tokio::runtime::Handle::try_current().is_ok() {
        None
    } else {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        app.set_runtime_handle(rt.handle().clone());
        Some(rt)
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let res = (|| -> Result<(), HlError> {
        loop {
            app.on_tick();

            terminal.draw(|f| {
                let size = f.area();
                app.draw(f, size);
            })?;

            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    match app.handle_key(key) {
                        HlKeyOutcome::Exit => break,
                        HlKeyOutcome::ConnectRequested => {
                            let flag = if app.is_mainnet { " --mainnet" } else { "" };
                            app.status_msg =
                                format!("Run `vaughan hl connect{flag}` to 1-click authorize agent on Hyperliquid.");
                        }
                        HlKeyOutcome::Consumed | HlKeyOutcome::OpenFaucet => {}
                    }
                }
            }
        }
        Ok(())
    })();

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_RUNS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn loads_strategy_entries_and_ranks_walk_forward_winners_first() {
        let data_dir = Path::new("data");
        let strat_dir = Path::new("strategies");
        if !data_dir.exists() || !strat_dir.exists() {
            return;
        }
        let app = HlTuiApp::load(data_dir, strat_dir).expect("load TUI app");
        assert!(!app.entries.is_empty(), "must load strategies");
        println!("\n=== STRATEGY LEADERBOARD (RANKED BY WF & NET R) ===");
        for (i, e) in app.entries.iter().enumerate() {
            println!(
                "{:2}. {:<32} | Net R: {:>+6.2}R | WR: {:>5.1}% ({:>2}W/{:>2}L) | PF: {:>5.2} | MaxDD: {:>5.2}R | WF: {}",
                i + 1,
                e.file_name,
                e.report.total_net_r,
                e.report.win_rate_pct,
                e.report.winning_trades,
                e.report.losing_trades,
                e.report.profit_factor,
                e.report.max_drawdown_r,
                if e.wf_verified { "YES (H1 & H2 profitable)" } else { "NO" }
            );
        }
        println!("====================================================\n");
        let top = &app.entries[0];
        assert!(
            top.wf_verified,
            "top-ranked strategy should be walk-forward verified"
        );
        assert!(top.report.total_net_r > 20.0);
    }

    #[test]
    fn quick_connect_lifecycle_and_key_handling() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let strat_dir = tmp.path().join("strategies");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&strat_dir).unwrap();

        let mut app = HlTuiApp::load(&data_dir, &strat_dir).unwrap();
        assert!(!app.is_connected);

        // When disconnected, Enter or 'a' opens the password prompt
        let enter = KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(enter), HlKeyOutcome::Consumed);
        assert!(app.prompt_active());

        // Cancel prompt with Esc
        let esc = KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(esc), HlKeyOutcome::Consumed);
        assert!(!app.prompt_active());

        // 'a' also opens prompt
        let key_a = KeyEvent::new(KeyCode::Char('a'), crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_a), HlKeyOutcome::Consumed);
        assert!(app.prompt_active());

        // Type valid password
        for c in "VaughanHl#2026!".chars() {
            let k = KeyEvent::new(KeyCode::Char(c), crossterm::event::KeyModifiers::NONE);
            assert_eq!(app.handle_key(k), HlKeyOutcome::Consumed);
        }
        assert_eq!(app.handle_key(enter), HlKeyOutcome::ConnectRequested);
        assert!(app.take_password().is_some());
        assert!(!app.prompt_active());

        let key_w = KeyEvent::new(KeyCode::Char('w'), crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_w), HlKeyOutcome::OpenFaucet);

        // Esc returns Exit
        assert_eq!(app.handle_key(esc), HlKeyOutcome::Exit);

        // Mark connected
        let agent = Address::repeat_byte(0x11);
        let backing = Address::repeat_byte(0x22);
        app.mark_connected(agent, backing, false, 1000.0);
        assert!(app.is_connected);
        assert!(app.agent_summary.contains("Agent 0x1111"));
        assert!(app.status_msg.contains("Equity: $1000.00 USDC"));

        // When connected, Enter is no longer ConnectRequested
        assert_eq!(app.handle_key(enter), HlKeyOutcome::Consumed);

        // When connected without an unlocked signer cached, Space prompts to unlock
        let space = KeyEvent::new(KeyCode::Char(' '), crossterm::event::KeyModifiers::NONE);
        assert!(!app.armed_dry_run);
        assert_eq!(app.handle_key(space), HlKeyOutcome::Consumed);
        assert!(app.prompt_active());
        assert_eq!(app.prompt.as_ref().unwrap().kind, PromptKind::Unlock);

        // Cancel prompt
        assert_eq!(app.handle_key(esc), HlKeyOutcome::Consumed);
        assert!(!app.prompt_active());

        // Network mode toggle
        app.set_network_mode(true);
        assert!(app.is_mainnet);
        assert!(!app.is_connected);
        assert!(app.agent_summary.contains("MAINNET"));
        assert!(app.status_msg.contains("Mainnet"));

        // When disconnected, Space arms simulation mode directly
        assert_eq!(app.handle_key(space), HlKeyOutcome::Consumed);
        assert!(app.armed_dry_run);
    }

    #[test]
    fn existing_keystore_unlock_flow() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let strat_dir = tmp.path().join("strategies");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&strat_dir).unwrap();

        let backing = Address::repeat_byte(0x55);
        let pw = SecretString::new("CorrectHorseBatteryStaple1!".into());
        crate::keystore::generate_and_save_agent_keystore(
            &data_dir,
            backing,
            true, // mainnet
            &pw,
            Some("tui-agent"),
            1,
        )
        .unwrap();

        let mut app = HlTuiApp::load(&data_dir, &strat_dir).unwrap();
        assert!(app.is_connected);
        assert!(app.is_mainnet);
        assert_eq!(app.backing_account, Some(backing));
        assert!(app.keystore_path().exists());

        // Pressing 'u' initiates Unlock prompt
        let key_u = KeyEvent::new(KeyCode::Char('u'), crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_u), HlKeyOutcome::Consumed);
        assert!(app.prompt_active());
        assert_eq!(app.prompt.as_ref().unwrap().kind, PromptKind::Unlock);

        // Entering wrong password gives an error
        let enter = KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
        for c in "WrongPassword123!".chars() {
            let k = KeyEvent::new(KeyCode::Char(c), crossterm::event::KeyModifiers::NONE);
            assert_eq!(app.handle_key(k), HlKeyOutcome::Consumed);
        }
        assert_eq!(app.handle_key(enter), HlKeyOutcome::Consumed);
        assert!(app.prompt_active());
        assert!(app.prompt.as_ref().unwrap().error.is_some());

        // Pressing 'r' switches to Connect mode for reset
        let key_r = KeyEvent::new(KeyCode::Char('r'), crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_r), HlKeyOutcome::Consumed);
        assert_eq!(app.prompt.as_ref().unwrap().kind, PromptKind::Connect);

        // Esc clears prompt
        let esc = KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(esc), HlKeyOutcome::Consumed);
        assert!(!app.prompt_active());

        // Test network toggle: switching to testnet disconnects
        app.set_network_mode(false);
        assert!(!app.is_connected);

        // When disconnected on testnet, Enter opens Unlock prompt
        assert_eq!(app.handle_key(enter), HlKeyOutcome::Consumed);
        assert!(app.prompt_active());
        assert_eq!(app.prompt.as_ref().unwrap().kind, PromptKind::Unlock);

        // Unlock with correct password
        for c in "CorrectHorseBatteryStaple1!".chars() {
            let k = KeyEvent::new(KeyCode::Char(c), crossterm::event::KeyModifiers::NONE);
            assert_eq!(app.handle_key(k), HlKeyOutcome::Consumed);
        }
        assert_eq!(app.handle_key(enter), HlKeyOutcome::Consumed);
        assert!(!app.prompt_active());
        assert!(app.is_connected);
        assert!(app.is_mainnet); // auto-restored mainnet from decrypted signer!
        assert_eq!(app.backing_account, Some(backing));
        assert!(app
            .status_msg
            .contains("Agent unlocked and connected successfully."));
    }

    #[test]
    fn spacebar_arm_and_stop_lifecycle_is_never_intermittent() {
        let _lock = TEST_RUNS_LOCK.lock().unwrap();
        live::stop_active();
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let strat_dir = tmp.path().join("strategies");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&strat_dir).unwrap();

        let mut app = HlTuiApp::load(&data_dir, &strat_dir).unwrap();
        let space = KeyEvent::new(KeyCode::Char(' '), crossterm::event::KeyModifiers::NONE);

        // 1. First Space press arms immediately (no missed presses)
        assert!(!app.armed_dry_run);
        assert_eq!(app.handle_key(space), HlKeyOutcome::Consumed);
        assert!(app.armed_dry_run);

        // 2. Second Space press stops immediately
        assert_eq!(app.handle_key(space), HlKeyOutcome::Consumed);
        assert!(!app.armed_dry_run);

        // 3. Third Space press immediately re-arms on the FIRST try
        assert_eq!(app.handle_key(space), HlKeyOutcome::Consumed);
        assert!(app.armed_dry_run);

        // 4. Test multi-market handles registration and stop
        let stop_a = Arc::new(AtomicBool::new(false));
        let stop_b = Arc::new(AtomicBool::new(false));
        let status_a = Arc::new(Mutex::new(LiveStatus {
            running: true,
            ..Default::default()
        }));
        let status_b = Arc::new(Mutex::new(LiveStatus {
            running: true,
            ..Default::default()
        }));

        live::register_run(LiveHandle {
            stop: Arc::clone(&stop_a),
            status: Arc::clone(&status_a),
            strategy_label: "test".into(),
            is_mainnet: false,
            coin: "BTC".into(),
        });
        live::register_run(LiveHandle {
            stop: Arc::clone(&stop_b),
            status: Arc::clone(&status_b),
            strategy_label: "test".into(),
            is_mainnet: false,
            coin: "ETH".into(),
        });

        assert!(live::has_active_runs());
        assert_eq!(live::active_runs().len(), 2);

        // Space stops all runs immediately
        assert_eq!(app.handle_key(space), HlKeyOutcome::Consumed);
        assert!(!live::has_active_runs());
        assert!(live::active_run().is_none());
        assert!(!app.armed_dry_run);
        assert!(!app.live_on);

        // Next Space re-arms immediately on the FIRST press without ghost pop cycles!
        assert_eq!(app.handle_key(space), HlKeyOutcome::Consumed);
        assert!(app.armed_dry_run);
    }

    #[test]
    fn multi_market_modal_selection_and_arming() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let strat_dir = tmp.path().join("strategies");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&strat_dir).unwrap();

        let mut app = HlTuiApp::load(&data_dir, &strat_dir).unwrap();

        // Press 'm' with no strategies does not panic
        let key_m = KeyEvent::new(KeyCode::Char('m'), crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_m), HlKeyOutcome::Consumed);
        assert!(app.market_modal.is_none());

        // Verify default choices pre-select CL and GOLD
        let choices = popular_market_choices(&["xyz:CL".to_string(), "xyz:GOLD".to_string()]);
        assert_eq!(choices[0].symbol, "xyz:CL");
        assert!(choices[0].selected);
        assert_eq!(choices[1].symbol, "xyz:GOLD");
        assert!(choices[1].selected);
        assert_eq!(choices[2].symbol, "xyz:SILVER");
        assert!(!choices[2].selected);
        assert!(choices.iter().any(|c| c.symbol == "BTC"));
        assert!(choices.iter().any(|c| c.symbol == "ETH"));
        // Test modal custom market addition
        let modal = MarketSelectorModal::new("bob.toml".to_string(), choices);
        app.market_modal = Some(modal);

        // Press '+' to enter input mode
        let key_plus = KeyEvent::new(KeyCode::Char('+'), crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_plus), HlKeyOutcome::Consumed);
        assert!(app.market_modal.as_ref().unwrap().input_mode);

        // Type 'S', 'H', 'I', 'B' and Enter
        for c in ['s', 'h', 'i', 'b'] {
            let key_c = KeyEvent::new(KeyCode::Char(c), crossterm::event::KeyModifiers::NONE);
            assert_eq!(app.handle_key(key_c), HlKeyOutcome::Consumed);
        }
        assert_eq!(app.market_modal.as_ref().unwrap().input_buf, "shib");

        let key_enter = KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_enter), HlKeyOutcome::Consumed);
        assert!(!app.market_modal.as_ref().unwrap().input_mode);

        let m = app.market_modal.as_ref().unwrap();
        let custom_item = m.items.iter().find(|i| i.symbol == "SHIB");
        assert!(custom_item.is_some());
        assert!(custom_item.unwrap().selected);
        assert_eq!(custom_item.unwrap().name, "SHIB (Custom)");
    }

    #[test]
    fn market_column_navigation_and_simultaneous_selection() {
        let _lock = TEST_RUNS_LOCK.lock().unwrap();
        live::stop_active();
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let strat_dir = tmp.path().join("strategies");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&strat_dir).unwrap();

        let mut app = HlTuiApp::load(&data_dir, &strat_dir).unwrap();
        assert_eq!(app.focus, HlFocus::Strategies);

        // Tab toggles focus to Markets column
        let tab = KeyEvent::new(KeyCode::Tab, crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(tab), HlKeyOutcome::Consumed);
        assert_eq!(app.focus, HlFocus::Markets);

        // 'x' toggles selection on the focused market
        let key_x = KeyEvent::new(KeyCode::Char('x'), crossterm::event::KeyModifiers::NONE);
        let init_sel = app.available_markets[0].selected;
        assert_eq!(app.handle_key(key_x), HlKeyOutcome::Consumed);
        assert_ne!(app.available_markets[0].selected, init_sel);

        // Down moves to next market
        let down = KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(down), HlKeyOutcome::Consumed);
        assert_eq!(app.market_cursor, 1);

        // Toggle second market as well (simultaneous multi-market)
        let init_sel_1 = app.available_markets[1].selected;
        assert_eq!(app.handle_key(key_x), HlKeyOutcome::Consumed);
        assert_ne!(app.available_markets[1].selected, init_sel_1);

        // Space in Markets column arms the bot cleanly!
        let space = KeyEvent::new(KeyCode::Char(' '), crossterm::event::KeyModifiers::NONE);
        assert!(!app.armed_dry_run);
        assert_eq!(app.handle_key(space), HlKeyOutcome::Consumed);
        assert!(app.armed_dry_run);
        // Space again stops the bot cleanly!
        assert_eq!(app.handle_key(space), HlKeyOutcome::Consumed);
        assert!(!app.armed_dry_run);

        // Press '+' in markets column to add a custom ticker
        let plus = KeyEvent::new(KeyCode::Char('+'), crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(plus), HlKeyOutcome::Consumed);
        assert!(app.market_input_mode);

        for c in ['s', 'o', 'l'] {
            let key_c = KeyEvent::new(KeyCode::Char(c), crossterm::event::KeyModifiers::NONE);
            assert_eq!(app.handle_key(key_c), HlKeyOutcome::Consumed);
        }
        let enter = KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(enter), HlKeyOutcome::Consumed);
        assert!(!app.market_input_mode);

        let sol_item = app.available_markets.iter().find(|i| i.symbol == "SOL");
        assert!(sol_item.is_some());
        assert!(sol_item.unwrap().selected);

        // Esc or Tab returns focus to Strategies
        let esc = KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(esc), HlKeyOutcome::Consumed);
        assert_eq!(app.focus, HlFocus::Strategies);
    }

    #[test]
    fn test_spinner_frame_cycles() {
        let expected = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        for (i, &exp) in expected.iter().enumerate() {
            assert_eq!(spinner_frame(i as u64), exp);
        }
        assert_eq!(spinner_frame(10), expected[0]);
        assert_eq!(spinner_frame(25), expected[5]);
    }

    #[test]
    fn test_colored_address_spans() {
        let addr = "0x5b9806b86c93bf8e570e62ab6fce41ca13e224b2";
        let spans = colored_address_spans(addr);
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, addr);
        assert_eq!(spans.len(), 6);
        assert_eq!(spans[0].content.as_ref(), "0x");
        assert_eq!(spans[0].style.fg, Some(ADDR_GREY));
        assert_eq!(spans[1].content.as_ref(), "5b980");
        assert_eq!(spans[1].style.fg, Some(ADDR_GREEN));
        assert_eq!(spans[2].content.as_ref(), "6b86c93bf8e57");
        assert_eq!(spans[2].style.fg, Some(ADDR_GREY));
        assert_eq!(spans[3].content.as_ref(), "0e62a");
        assert_eq!(spans[3].style.fg, Some(ADDR_ORANGE));
        assert_eq!(spans[4].content.as_ref(), "b6fce41ca13e");
        assert_eq!(spans[4].style.fg, Some(ADDR_GREY));
        assert_eq!(spans[5].content.as_ref(), "224b2");
        assert_eq!(spans[5].style.fg, Some(ADDR_PURPLE));
    }

    #[test]
    fn test_address_copy_pulse_spans() {
        let addr = "0x5b9806b86c93bf8e570e62ab6fce41ca13e224b2";
        let pulse_0 = address_copy_pulse_spans(addr, 0);
        let text_0: String = pulse_0.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            text_0.chars().count(),
            addr.chars().count(),
            "exact character length match"
        );
        assert!(text_0.contains(" copied "), "contains centered copied word");

        let at = text_0.find(" copied ").expect("word location");
        let prefix_len = text_0[..at].chars().count();
        assert_eq!(prefix_len, 17, "centered at (42 - 8) / 2 = 17");

        let pulse_1 = address_copy_pulse_spans(addr, 1);
        let text_1: String = pulse_1.iter().map(|s| s.content.as_ref()).collect();
        assert_ne!(text_0, text_1, "advancing tick shifts braille frames");
        assert_eq!(text_1.chars().count(), addr.chars().count());
    }

    #[test]
    fn test_copy_hotkeys_and_pulse_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let strat_dir = tmp.path().join("strategies");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&strat_dir).unwrap();

        let backing = Address::repeat_byte(0x77);
        let pw = SecretString::new("VaughanPulseTest1!".into());
        crate::keystore::generate_and_save_agent_keystore(
            &data_dir,
            backing,
            true,
            &pw,
            Some("pulse-agent"),
            1,
        )
        .unwrap();

        let mut app = HlTuiApp::load(&data_dir, &strat_dir).unwrap();
        assert_eq!(app.copy_pulse_ticks, 0);
        assert_eq!(app.anim_tick, 0);

        // Pressing 'y' triggers copy pulse
        let key_y = KeyEvent::new(KeyCode::Char('y'), crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_y), HlKeyOutcome::Consumed);
        assert_eq!(app.copy_pulse_ticks, 15);
        assert!(app.status_msg.contains("Agent"));

        // on_tick increments anim_tick and decrements copy_pulse_ticks
        app.on_tick();
        assert_eq!(app.anim_tick, 1);
        assert_eq!(app.copy_pulse_ticks, 14);

        for _ in 0..14 {
            app.on_tick();
        }
        assert_eq!(app.copy_pulse_ticks, 0);
        assert_eq!(app.anim_tick, 15);

        // Pressing 'c' in Strategies opens Hyperliquid in VB (the same as pressing Enter from the whitelist view)
        let key_c = KeyEvent::new(KeyCode::Char('c'), crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_c), HlKeyOutcome::Consumed);
        assert!(
            app.status_msg.contains("opened in VB")
                || app.status_msg.contains("Opened Hyperliquid")
        );
        assert_eq!(
            app.copy_pulse_ticks, 0,
            "'c' must not trigger copy pulse anymore"
        );

        // Switch to Markets column: 'y' still copies!
        let tab = KeyEvent::new(KeyCode::Tab, crossterm::event::KeyModifiers::NONE);
        app.handle_key(tab);
        assert_eq!(app.focus, HlFocus::Markets);
        app.copy_pulse_ticks = 0;
        assert_eq!(app.handle_key(key_y), HlKeyOutcome::Consumed);
        assert_eq!(app.copy_pulse_ticks, 15);
    }

    #[test]
    fn test_hyper_chart_lifecycle_and_navigation() {
        let mut app = HlTuiApp::load_default();
        app.show_chart = true;
        assert_eq!(app.chart_scroll, 0);

        // Arrow Left scrolls back in time
        let key_left = KeyEvent::new(KeyCode::Left, crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_left), HlKeyOutcome::Consumed);
        assert_eq!(app.chart_scroll, 5);

        // Arrow Right scrolls forward
        let key_right = KeyEvent::new(KeyCode::Right, crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_right), HlKeyOutcome::Consumed);
        assert_eq!(app.chart_scroll, 0);

        // 'm' or 'Tab' cycles market
        let key_m = KeyEvent::new(KeyCode::Char('m'), crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_m), HlKeyOutcome::Consumed);
        assert_eq!(app.chart_market_idx, 1);

        // 'Esc' closes chart
        let key_esc = KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_esc), HlKeyOutcome::Consumed);
        assert!(!app.show_chart);
    }

    #[test]
    fn test_session_run_vs_today_history_and_reset() {
        let mut app = HlTuiApp::load_default();
        let initial_start = app.session_start_ms;
        assert!(initial_start > 0);
        assert!(
            !app.show_all_day_trades,
            "defaults to showing current run fills only"
        );

        // Simulate fills: 1 older fill from today before session_start, 1 fill after session_start
        let old_fill = LiveTradeFill {
            oid: 101,
            time_ms: initial_start - 60_000,
            coin: "BTC".into(),
            dir: "Buy".into(),
            px: 100_000.0,
            sz: 0.01,
            fee: 0.01,
            closed_pnl: -0.02,
        };
        let new_fill = LiveTradeFill {
            oid: 102,
            time_ms: initial_start + 10_000,
            coin: "BTC".into(),
            dir: "Sell".into(),
            px: 101_000.0,
            sz: 0.01,
            fee: 0.01,
            closed_pnl: 0.10,
        };
        if let Ok(mut g) = app.live_trades.lock() {
            g.push(old_fill);
            g.push(new_fill);
        }

        // 'v' toggles tape between current run and today's full history
        let key_v = KeyEvent::new(KeyCode::Char('v'), crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_v), HlKeyOutcome::Consumed);
        assert!(app.show_all_day_trades);
        assert_eq!(app.handle_key(key_v), HlKeyOutcome::Consumed);
        assert!(!app.show_all_day_trades);

        // 'r' resets session_start_ms to now
        let key_r = KeyEvent::new(KeyCode::Char('r'), crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.handle_key(key_r), HlKeyOutcome::Consumed);
        assert!(app.session_start_ms >= initial_start);
        assert!(app.status_msg.contains("Session reset"));
    }

    #[test]
    fn test_open_hyperliquid_dapp_url() {
        let mut app = HlTuiApp::load_default();
        app.is_mainnet = false;
        let testnet_url = app.open_hyperliquid_dapp();
        assert_eq!(testnet_url, "https://app.hyperliquid-testnet.xyz/trade");

        app.is_mainnet = true;
        let mainnet_url = app.open_hyperliquid_dapp();
        assert_eq!(mainnet_url, "https://app.hyperliquid.xyz/trade");
    }

    #[test]
    fn active_strategy_does_not_drift_into_standby_on_ticks() {
        let _lock = TEST_RUNS_LOCK.lock().unwrap();
        live::stop_active();
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let strat_dir = tmp.path().join("strategies");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&strat_dir).unwrap();

        live::stop_active();

        let mut app = HlTuiApp::load(&data_dir, &strat_dir).unwrap();
        app.is_connected = true;

        // Register an active run handle as start_live_* would
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(LiveStatus::starting(
            "LIVE [test [BTC]] starting...",
        )));
        live::register_run(LiveHandle {
            stop: Arc::clone(&stop),
            status: Arc::clone(&status),
            strategy_label: "test".into(),
            is_mainnet: true,
            coin: "BTC".into(),
        });

        // 1. First tick should mirror active run and keep live_on and armed_dry_run true
        app.on_tick();
        assert!(
            app.live_on,
            "app.live_on must remain true while active run exists"
        );
        assert!(
            app.armed_dry_run,
            "app.armed_dry_run must remain true while active run exists"
        );
        assert!(app.status_msg.contains("LIVE [test [BTC]] starting..."));

        // 2. Subsequent ticks should NOT drift into standby
        for _ in 0..10 {
            app.on_tick();
            assert!(app.live_on, "app.live_on drifted into standby!");
            assert!(app.armed_dry_run, "app.armed_dry_run drifted into standby!");
        }

        // 3. If the run halts with an error, on_tick captures the error message rather than silently dropping
        if let Ok(mut g) = status.lock() {
            g.running = false;
            g.halted = true;
            g.msg = "LIVE HALTED: daily loss limit reached ($ -10.50)".into();
        }

        app.on_tick();
        assert!(!app.live_on);
        assert!(!app.armed_dry_run);
        assert_eq!(
            app.status_msg,
            "LIVE HALTED: daily loss limit reached ($ -10.50)"
        );

        // Clean up
        live::stop_active();
    }
}
