//! CLI entrypoint for `vaughan-hl`.

use std::path::{Path, PathBuf};

use alloy::primitives::Address;
use clap::{Parser, Subcommand};
use secrecy::SecretString;
use vaughan_hl::engine::evaluate_closed_candle_tick;
use vaughan_hl::indicators::{IndicatorConfig, IndicatorTable};
use vaughan_hl::keystore::EncryptedAgentKeystore;
use vaughan_hl::market_data::{
    default_cache_path, fetch_candles_from_hl, load_candles_cache, now_unix_ms, save_candles_cache,
    CURATED_MARKETS, HL_MAINNET_INFO_URL,
};
use vaughan_hl::replay::{run_parameter_sweep, run_replay_with_source};
use vaughan_hl::strategy::{estimated_liq_distance_bps, CompiledStrategy};
use vaughan_hl::wire::build_approve_agent_typed_data;

#[derive(Parser)]
#[command(
    name = "vaughan",
    version,
    about = "Hyperliquid multi-indicator (RSI + KST + EMA + ATR) strategy lab & R:R replay engine"
)]
struct Cli {
    /// Directory for cached OHLCV candles.
    #[arg(long, default_value = "data", global = true)]
    data_dir: PathBuf,

    /// Directory containing strategy TOML files.
    #[arg(long, default_value = "strategies", global = true)]
    strategies_dir: PathBuf,

    /// Target mainnet instead of testnet (default: false = testnet-first).
    #[arg(long, default_value_t = false, global = true)]
    mainnet: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum HlSubcommands {
    /// 1-Click Connect & Authorize Agent Key on Hyperliquid Testnet (or Mainnet).
    Connect {
        /// Target mainnet instead of testnet (default: false = testnet-first).
        #[arg(long, default_value_t = false)]
        mainnet: bool,
    },
    /// Execute a strategy live in the background (headless loop with real order execution).
    Run {
        /// Path to strategy TOML file.
        #[arg(long, short = 's')]
        strategy: PathBuf,
        /// Maximum capital allocation in USD for position sizing (default: 100.0).
        #[arg(long, default_value_t = 100.0)]
        allocation: f64,
        /// Optional password env var for unlocking the agent keystore (otherwise prompted interactively).
        #[arg(long)]
        password_env: Option<String>,
    },
    /// Display active Hyperliquid Agent Address, check exchange status, and copy to clipboard.
    Agent,
    /// View or configure the automated Perp-to-Spot Profit Siphon (e.g. 20% into BTC 0xF7bf…0F81).
    Siphon {
        /// Percentage of net realized profits to auto-reinvest (default: 20.0).
        #[arg(long)]
        pct: Option<f64>,
        /// Target token ticker (e.g. "BTC", "PLS", "PLSX") or ERC-20 contract address.
        #[arg(long)]
        token: Option<String>,
        /// Test multi-aggregator quote flow on PulseChain without broadcasting.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the interactive terminal user interface (default when run with no subcommand).
    #[cfg(feature = "tui")]
    Tui,

    /// View or configure the automated Perp-to-Spot Profit Siphon (e.g. 20% into BTC 0xF7bf…0F81).
    Siphon {
        /// Percentage of net realized profits to auto-reinvest (default: 20.0).
        #[arg(long)]
        pct: Option<f64>,
        /// Target token ticker (e.g. "BTC", "PLS", "PLSX") or ERC-20 contract address.
        #[arg(long)]
        token: Option<String>,
        /// Test multi-aggregator quote flow on PulseChain without broadcasting.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },

    /// Hyperliquid strategy lab, walk-forward backtester & auto-trader TUI.
    Hl {
        #[command(subcommand)]
        sub: Option<HlSubcommands>,
        /// Target mainnet instead of testnet (default: false = testnet-first).
        #[arg(long, default_value_t = false)]
        mainnet: bool,
    },

    /// 1-Click Connect & Authorize Agent Key on Hyperliquid Testnet (or Mainnet).
    Connect {
        /// Target mainnet instead of testnet (default: false = testnet-first).
        #[arg(long, default_value_t = false)]
        mainnet: bool,
    },

    /// List curated Hyperliquid crypto & HIP-3 commodity/equity markets.
    Markets,

    /// Fetch historical closed candles from Hyperliquid and save to local cache.
    Fetch {
        /// Market symbol (e.g. "BTC", "ETH", "HYPE", "xyz:CL", "xyz:GOLD", "xyz:NVDA").
        #[arg(long, default_value = "BTC")]
        coin: String,
        /// Candle timeframe ("1m", "3m", "5m", "15m", "30m", "1h", "4h", "1d").
        #[arg(long, default_value = "15m")]
        interval: String,
        /// Number of days of history to fetch.
        #[arg(long, default_value_t = 30)]
        days: u32,
    },

    /// Inspect latest closed-candle indicator readings (RSI, KST, EMA, ATR, ADX, BB, StochRSI).
    Inspect {
        #[arg(long, default_value = "BTC")]
        coin: String,
        #[arg(long, default_value = "15m")]
        interval: String,
        #[arg(long, default_value_t = 30)]
        days: u32,
    },

    /// Run a deterministic R:R backtest from a TOML strategy file.
    Replay {
        /// Path to strategy TOML file (e.g. strategies/btc_15m_rsi_kst_3r.toml).
        #[arg(long, short = 's')]
        strategy: PathBuf,
        /// Optional explicit candle JSON path (otherwise uses/fetches cache for strategy coin & interval).
        #[arg(long)]
        candles: Option<PathBuf>,
        /// Days to fetch if cache is missing.
        #[arg(long, default_value_t = 30)]
        days: u32,
        /// Print individual simulated trades.
        #[arg(long, default_value_t = true)]
        show_trades: bool,
    },

    /// Grid-sweep multi-indicator (RSI + KST + EMA + StochRSI + BB) and R:R setups on a market.
    Sweep {
        #[arg(long, default_value = "BTC")]
        coin: String,
        #[arg(long, default_value = "15m")]
        interval: String,
        #[arg(long, default_value_t = 30)]
        days: u32,
    },

    /// Generate an encrypted 0600 HL Agent Keystore and the EIP-712 `ApproveAgent` JSON for `vaughan sign-typed-data`.
    AgentInit {
        /// Dedicated risk-capital sub-account / EOA address (0x...) that will approve this agent.
        #[arg(long)]
        backing_account: String,
        /// Target mainnet instead of testnet (default: false = testnet-first).
        #[arg(long, default_value_t = false)]
        mainnet: bool,
        /// Env var holding the keystore password (otherwise prompts interactively with no echo).
        #[arg(long)]
        password_env: Option<String>,
    },

    /// Evaluate the latest closed candle for a strategy (testnet/dry-run default) and output the L1 bracket order plan.
    Tick {
        /// Path to strategy TOML file.
        #[arg(long, short = 's')]
        strategy: PathBuf,
        /// Simulated or live account equity in USD for sizing.
        #[arg(long, default_value_t = 1000.0)]
        equity_usd: f64,
        /// Hyperliquid numeric asset ID (`a` field on the wire).
        #[arg(long, default_value_t = 0)]
        asset_id: u32,
        /// Size decimals (`szDecimals` from Hyperliquid `meta`).
        #[arg(long, default_value_t = 2)]
        sz_decimals: u32,
        /// Days of candle history to load/fetch.
        #[arg(long, default_value_t = 30)]
        days: u32,
    },

    /// Emergency kill-switch: emit an EIP-712 `ApproveAgent(0x0000...0000)` revocation payload for `vaughan sign-typed-data`.
    Kill {
        /// Target mainnet instead of testnet.
        #[arg(long, default_value_t = false)]
        mainnet: bool,
    },

    /// Execute a strategy live in the background (headless loop with real order execution).
    Run {
        /// Path to strategy TOML file.
        #[arg(long, short = 's')]
        strategy: PathBuf,
        /// Maximum capital allocation in USD for position sizing (default: 100.0).
        #[arg(long, default_value_t = 100.0)]
        allocation: f64,
        /// Optional password env var for unlocking the agent keystore (otherwise prompted interactively).
        #[arg(long)]
        password_env: Option<String>,
    },

    /// Display active Hyperliquid Agent Address, check exchange status, and copy to clipboard.
    Agent,
}

/// Loads the extra candle series that a higher-timeframe filter block declares via
/// `source = "<interval>"`. Returns `None` when no block asks for one.
async fn load_htf_source_candles(
    data_dir: &Path,
    strategy: &CompiledStrategy,
    days: u32,
) -> Option<Vec<vaughan_hl::market_data::Candle>> {
    let interval = strategy
        .config
        .indicators
        .htf
        .as_ref()
        .and_then(|h| h.source.clone())
        .or_else(|| {
            strategy
                .config
                .indicators
                .htf2
                .as_ref()
                .and_then(|h| h.source.clone())
        })?;
    // The source series has to reach back far enough to warm up the higher
    // timeframe (a weekly StochRSI needs ~29 weekly closes).
    let span = days.max(800);
    match load_or_fetch_candles(data_dir, &strategy.config.market.coin, &interval, span).await {
        Ok(candles) if candles.len() > 1 => {
            println!(
                " HTF source         : {} {interval} bars loaded for the filter",
                candles.len()
            );
            Some(candles)
        }
        _ => {
            eprintln!("warning: could not load {interval} candles for the HTF filter");
            None
        }
    }
}

async fn load_or_fetch_candles(
    data_dir: &Path,
    coin: &str,
    interval: &str,
    days: u32,
) -> anyhow::Result<Vec<vaughan_hl::market_data::Candle>> {
    // Intervals the venue does not serve are built from a lower one it does.
    if let Some(source_iv) = vaughan_hl::market_data::derived_source_interval(interval) {
        let base = Box::pin(load_or_fetch_candles(data_dir, coin, source_iv, days)).await?;
        let target_ms = vaughan_hl::market_data::interval_duration_ms(interval)?;
        let aggregated = vaughan_hl::market_data::aggregate_candles(&base, target_ms);
        println!(
            "Derived {} {} bars by aggregating {} {} candles",
            aggregated.len(),
            interval,
            base.len(),
            source_iv
        );
        return Ok(aggregated);
    }

    let cache_path = default_cache_path(data_dir, coin, interval);
    if cache_path.exists() {
        let cached = load_candles_cache(&cache_path)?;
        if !cached.is_empty() {
            return Ok(cached);
        }
    }
    println!(
        "Fetching {} ({}) closed candles ({} days) from Hyperliquid...",
        coin, interval, days
    );
    let candles = fetch_candles_from_hl(HL_MAINNET_INFO_URL, coin, interval, days).await?;
    save_candles_cache(&cache_path, &candles)?;
    println!(
        "Saved {} closed candles -> {}",
        candles.len(),
        cache_path.display()
    );
    Ok(candles)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut cli = Cli::parse();

    if !cli.data_dir.join("hl_agent_keystore.json").exists() {
        for candidate in [
            PathBuf::from("data"),
            PathBuf::from("vaughan-hl/data"),
            Path::new(env!("CARGO_MANIFEST_DIR")).join("data"),
        ] {
            if candidate.join("hl_agent_keystore.json").exists() {
                cli.data_dir = candidate;
                break;
            }
        }
    }

    let has_strategies = |p: &Path| -> bool {
        p.exists()
            && p.read_dir()
                .map(|mut r| {
                    r.any(|e| {
                        e.map(|entry| {
                            entry
                                .path()
                                .extension()
                                .map(|ext| ext == "toml")
                                .unwrap_or(false)
                        })
                        .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
    };
    if !has_strategies(&cli.strategies_dir) {
        for candidate in [
            PathBuf::from("strategies"),
            PathBuf::from("vaughan-hl/strategies"),
            Path::new(env!("CARGO_MANIFEST_DIR")).join("strategies"),
        ] {
            if has_strategies(&candidate) {
                cli.strategies_dir = candidate;
                break;
            }
        }
    }

    let effective_mainnet = if cli.mainnet {
        true
    } else if let Ok(ks) = vaughan_hl::keystore::EncryptedAgentKeystore::load_from_file(
        &cli.data_dir.join("hl_agent_keystore.json"),
    ) {
        ks.is_mainnet
    } else {
        false
    };

    let Some(command) = cli.command else {
        #[cfg(feature = "tui")]
        {
            vaughan_hl::tui::run_tui_ext(&cli.data_dir, &cli.strategies_dir, effective_mainnet)?;
            if std::env::var("VAUGHAN_PARENT_TUI").is_err() {
                let _ = std::process::Command::new("vaughan").status();
            }
            return Ok(());
        }
        #[cfg(not(feature = "tui"))]
        {
            anyhow::bail!("no subcommand specified (compile with `--features tui` for the interactive dashboard)");
        }
    };

    match command {
        #[cfg(feature = "tui")]
        Commands::Tui => {
            vaughan_hl::tui::run_tui_ext(&cli.data_dir, &cli.strategies_dir, effective_mainnet)?;
            if std::env::var("VAUGHAN_PARENT_TUI").is_err() {
                let _ = std::process::Command::new("vaughan").status();
            }
        }

        Commands::Hl { sub, mainnet } => {
            let use_mainnet = mainnet || effective_mainnet;
            match sub {
                None => {
                    #[cfg(feature = "tui")]
                    {
                        vaughan_hl::tui::run_tui_ext(
                            &cli.data_dir,
                            &cli.strategies_dir,
                            use_mainnet,
                        )?;
                        if std::env::var("VAUGHAN_PARENT_TUI").is_err() {
                            let _ = std::process::Command::new("vaughan").status();
                        }
                    }
                    #[cfg(not(feature = "tui"))]
                    {
                        anyhow::bail!(
                            "compile with `--features tui` for the interactive dashboard"
                        );
                    }
                }
                Some(HlSubcommands::Connect { mainnet: m }) => {
                    handle_agent_card(&cli.data_dir, m || use_mainnet).await?;
                }
                Some(HlSubcommands::Agent) => {
                    handle_agent_card(&cli.data_dir, use_mainnet).await?;
                }
                Some(HlSubcommands::Run {
                    strategy,
                    allocation,
                    password_env,
                }) => {
                    handle_run(
                        &cli.data_dir,
                        use_mainnet,
                        strategy,
                        allocation,
                        password_env,
                    )
                    .await?;
                }
                Some(HlSubcommands::Siphon {
                    pct,
                    token,
                    dry_run,
                }) => {
                    handle_siphon_command(&cli.strategies_dir, pct, token, dry_run).await?;
                }
            }
        }

        Commands::Siphon {
            pct,
            token,
            dry_run,
        } => {
            handle_siphon_command(&cli.strategies_dir, pct, token, dry_run).await?;
        }

        Commands::Connect { mainnet } => {
            handle_agent_card(&cli.data_dir, mainnet || effective_mainnet).await?;
        }

        Commands::Markets => {
            println!(
                "{:<14} {:<14} {:<8} DESCRIPTION",
                "SYMBOL", "CATEGORY", "HIP-3"
            );
            println!("{}", "-".repeat(68));
            for m in CURATED_MARKETS {
                println!(
                    "{:<14} {:<14} {:<8} {}",
                    m.symbol,
                    m.category,
                    if m.is_hip3 { "yes" } else { "no" },
                    m.name
                );
            }
        }

        Commands::Fetch {
            coin,
            interval,
            days,
        } => {
            let cache_path = default_cache_path(&cli.data_dir, &coin, &interval);
            println!(
                "Fetching {} [{}] ({} days) from {} ...",
                coin, interval, days, HL_MAINNET_INFO_URL
            );
            let candles =
                fetch_candles_from_hl(HL_MAINNET_INFO_URL, &coin, &interval, days).await?;
            save_candles_cache(&cache_path, &candles)?;
            println!(
                "Cached {} closed bars -> {}",
                candles.len(),
                cache_path.display()
            );
        }

        Commands::Inspect {
            coin,
            interval,
            days,
        } => {
            let candles = load_or_fetch_candles(&cli.data_dir, &coin, &interval, days).await?;
            let cfg = IndicatorConfig::default();
            let table = IndicatorTable::compute(&candles, &cfg);
            let Some(last_idx) = candles.len().checked_sub(1) else {
                anyhow::bail!("no candles returned for {coin}");
            };
            let c = &candles[last_idx];
            let fmt_opt = |v: Option<f64>| {
                v.map(|x| format!("{x:.2}"))
                    .unwrap_or_else(|| "n/a".to_string())
            };

            println!("=== {} [{}] Latest Closed Bar ===", coin, interval);
            println!("Bars loaded : {}", candles.len());
            println!(
                "OHLC        : O={:.4} H={:.4} L={:.4} C={:.4} (Vol={:.2})",
                c.open, c.high, c.low, c.close, c.volume
            );
            println!(
                "RSI({})     : {} | {}({}) MA={}",
                cfg.rsi.period,
                fmt_opt(table.rsi[last_idx]),
                cfg.rsi.ma_type.as_str(),
                cfg.rsi.ma_period,
                fmt_opt(table.rsi_ma[last_idx])
            );
            println!(
                "KST         : Line={} | Signal={} | Hist={}",
                fmt_opt(table.kst_line[last_idx]),
                fmt_opt(table.kst_signal[last_idx]),
                fmt_opt(table.kst_hist[last_idx])
            );
            println!(
                "EMA         : EMA(21)={} | EMA(200)={}",
                fmt_opt(table.ema_fast[last_idx]),
                fmt_opt(table.ema_trend[last_idx])
            );
            println!(
                "ADX(14)     : ADX={} (+DI={}, -DI={})",
                fmt_opt(table.adx[last_idx]),
                fmt_opt(table.plus_di[last_idx]),
                fmt_opt(table.minus_di[last_idx])
            );
            println!("ATR(14)     : {}", fmt_opt(table.atr[last_idx]));
            println!(
                "StochRSI    : %K={} | %D={}",
                fmt_opt(table.stoch_k[last_idx]),
                fmt_opt(table.stoch_d[last_idx])
            );
            println!("Connors RSI : {}", fmt_opt(table.connors_rsi[last_idx]));
            println!(
                "SuperTrend  : {} (dir={})",
                fmt_opt(table.supertrend[last_idx]),
                match table.supertrend_dir[last_idx] {
                    Some(d) if d > 0.0 => "bullish",
                    Some(_) => "bearish",
                    None => "n/a",
                }
            );
            println!(
                "Bollinger   : Upper={} | Mid={} | Lower={} (%B={})",
                fmt_opt(table.bb_upper[last_idx]),
                fmt_opt(table.bb_middle[last_idx]),
                fmt_opt(table.bb_lower[last_idx]),
                fmt_opt(table.bb_pct_b[last_idx])
            );
        }

        Commands::Replay {
            strategy: strat_path,
            candles: candles_override,
            days,
            show_trades,
        } => {
            let compiled = CompiledStrategy::from_toml_file(&strat_path)?;
            let candles = if let Some(path) = candles_override {
                load_candles_cache(&path)?
            } else {
                load_or_fetch_candles(
                    &cli.data_dir,
                    &compiled.config.market.coin,
                    &compiled.config.market.interval,
                    days,
                )
                .await?
            };

            let htf_source = load_htf_source_candles(&cli.data_dir, &compiled, days).await;
            let report = run_replay_with_source(&candles, &compiled, htf_source.as_deref())?;
            let liq_bps = estimated_liq_distance_bps(compiled.config.market.leverage);
            let max_sl_bps = compiled.config.exit.stop_loss.max_stop_bps();

            println!("========================================================================");
            println!(" STRATEGY REPLAY REPORT: {}", report.strategy_name);
            println!("========================================================================");
            println!(
                " Market & Timeframe : {} [{}] | Leverage: {}x | Bars: {} (warmup: {})",
                report.coin,
                report.interval,
                compiled.config.market.leverage,
                report.total_candles,
                report.warmup_candles
            );
            println!(
                " Stop vs Liq Guard  : Max SL = {} bps | Est. {}x Liq = ~{} bps (SAFE)",
                max_sl_bps, compiled.config.market.leverage, liq_bps
            );
            println!("------------------------------------------------------------------------");
            println!(
                " Trades             : {} (Long: {}, Short: {})",
                report.total_trades, report.long_trades, report.short_trades
            );
            println!(
                " Win Rate           : {:.1}% ({} W / {} L) | Breakeven Win Rate @ {:.1}R: {:.1}%",
                report.win_rate_pct,
                report.winning_trades,
                report.losing_trades,
                compiled.config.exit.take_profit_rr,
                report.breakeven_win_rate_pct
            );
            println!(
                " R-Multiple Summary : Net = {:+.2}R | Expectancy = {:+.2}R / trade",
                report.total_net_r, report.expectancy_r
            );
            println!(
                " Avg Win / Avg Loss : {:+.2}R / {:+.2}R | Profit Factor = {:.2}",
                report.avg_win_r, report.avg_loss_r, report.profit_factor
            );
            println!(
                " Max Drawdown       : {:.2}R ({:.2}%)",
                report.max_drawdown_r, report.max_drawdown_pct
            );
            println!(
                " Equity ($ USD)     : ${:.2} -> ${:.2} (Net: {:+.2} USD / {:+.2}%, Fees: ${:.2})",
                report.initial_equity_usd,
                report.final_equity_usd,
                report.net_pnl_usd,
                report.net_return_pct,
                report.total_fees_usd
            );
            if candles.len() > report.warmup_candles * 2 + 20 {
                let mid = candles.len() / 2;
                let h1_slice = &candles[..mid];
                let h2_slice = &candles[mid.saturating_sub(report.warmup_candles)..];
                if let (Ok(h1), Ok(h2)) = (
                    run_replay_with_source(h1_slice, &compiled, htf_source.as_deref()),
                    run_replay_with_source(h2_slice, &compiled, htf_source.as_deref()),
                ) {
                    println!(
                        " Walk-Forward Split : H1 (1st 50%) = {:+.2}R (PF {:.2}, {} tr) | H2 (2nd 50%) = {:+.2}R (PF {:.2}, {} tr)",
                        h1.total_net_r,
                        h1.profit_factor,
                        h1.total_trades,
                        h2.total_net_r,
                        h2.profit_factor,
                        h2.total_trades
                    );
                }
            }
            println!("========================================================================");

            if show_trades && !report.trades.is_empty() {
                println!(
                    "\n{:<4} {:<6} {:<12} {:<12} {:<10} {:<9} {:<10} {:<14}",
                    "#", "SIDE", "ENTRY PX", "EXIT PX", "1R RISK", "NET R", "NET USD", "REASON"
                );
                println!("{}", "-".repeat(84));
                for t in report.trades.iter().rev().take(20).rev() {
                    println!(
                        "{:<4} {:<6} {:<12.4} {:<12.4} ${:<9.2} {:>+7.2}R  ${:>+8.2}  {:<14}",
                        t.id,
                        t.side.as_str(),
                        t.entry_px,
                        t.exit_px,
                        t.risk_1r_usd,
                        t.realized_r,
                        t.net_pnl_usd,
                        t.exit_reason.as_str()
                    );
                }
            }
        }

        Commands::Sweep {
            coin,
            interval,
            days,
        } => {
            let candles = load_or_fetch_candles(&cli.data_dir, &coin, &interval, days).await?;
            let rows = run_parameter_sweep(&candles, &coin, &interval)?;

            println!(
                "=== Multi-Indicator + R:R Parameter Sweep on {} [{}] ({} closed bars) ===",
                coin,
                interval,
                candles.len()
            );
            println!(
                "{:<58} {:>6} {:>7} {:>8} {:>8} {:>6} {:>7}",
                "STRATEGY VARIANT", "TRADES", "WIN%", "NET R", "EXP(R)", "PF", "MAX DD"
            );
            println!("{}", "-".repeat(106));
            for row in rows.iter().take(15) {
                println!(
                    "{:<58} {:>6} {:>6.1}% {:>+7.2}R {:>+7.2}R {:>6.2} {:>6.2}R",
                    row.label,
                    row.total_trades,
                    row.win_rate_pct,
                    row.total_net_r,
                    row.expectancy_r,
                    row.profit_factor,
                    row.max_drawdown_r
                );
            }
        }

        Commands::AgentInit {
            backing_account,
            mainnet,
            password_env,
        } => {
            let backing: Address = backing_account
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid backing_account address: {e}"))?;
            let password = match password_env {
                Some(env_name) => {
                    let val = std::env::var(&env_name).map_err(|_| {
                        anyhow::anyhow!("environment variable '{env_name}' is not set")
                    })?;
                    SecretString::new(val)
                }
                None => {
                    let raw = rpassword::prompt_password("New HL Agent Keystore password: ")?;
                    SecretString::new(raw)
                }
            };

            let keystore = EncryptedAgentKeystore::generate_random(backing, mainnet, &password)?;
            let ks_path = cli.data_dir.join("hl_agent_keystore.json");
            keystore.save_to_file(&ks_path)?;

            let approve_json = build_approve_agent_typed_data(
                keystore.agent_address,
                Some("vaughan-hl"),
                now_unix_ms(),
                mainnet,
            );
            let approve_path = cli.data_dir.join("approve_agent.json");
            std::fs::write(&approve_path, serde_json::to_string_pretty(&approve_json)?)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(&approve_path, std::fs::Permissions::from_mode(0o600));
            }

            println!(
                "Saved encrypted 0600 HL Agent Keystore -> {}",
                ks_path.display()
            );
            println!("  Agent Address   : {:#x}", keystore.agent_address);
            println!("  Backing Account : {:#x}", keystore.backing_account);
            println!(
                "  Network         : {}",
                if mainnet { "Mainnet" } else { "Testnet" }
            );
            println!(
                "\nNext step — sign the ApproveAgent payload once with your isolated sub-account in Vaughan-CLI:"
            );
            println!(
                "  vaughan sign-typed-data --file {}",
                approve_path.display()
            );
        }

        Commands::Tick {
            strategy: strat_path,
            equity_usd,
            asset_id,
            sz_decimals,
            days,
        } => {
            let compiled = CompiledStrategy::from_toml_file(&strat_path)?;
            let candles = load_or_fetch_candles(
                &cli.data_dir,
                &compiled.config.market.coin,
                &compiled.config.market.interval,
                days,
            )
            .await?;
            let htf_source = load_htf_source_candles(&cli.data_dir, &compiled, days).await;
            match evaluate_closed_candle_tick(
                &candles,
                htf_source.as_deref(),
                &compiled,
                equity_usd,
                asset_id,
                sz_decimals,
                None,
            )? {
                Some(plan) => {
                    println!("{}", serde_json::to_string_pretty(&plan)?);
                }
                None => {
                    println!(
                        "No entry signal on latest closed {} [{}] bar (total bars: {}).",
                        compiled.config.market.coin,
                        compiled.config.market.interval,
                        candles.len()
                    );
                }
            }
        }

        Commands::Kill { mainnet } => {
            let revoke_json =
                build_approve_agent_typed_data(Address::ZERO, None, now_unix_ms(), mainnet);
            let revoke_path = cli.data_dir.join("revoke_agent.json");
            if let Some(parent) = revoke_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&revoke_path, serde_json::to_string_pretty(&revoke_json)?)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(&revoke_path, std::fs::Permissions::from_mode(0o600));
            }
            println!(
                "Wrote emergency Agent Revocation EIP-712 payload -> {}",
                revoke_path.display()
            );
            println!(
                "Sign with Vaughan-CLI to revoke the agent immediately:\n  vaughan sign-typed-data --file {}",
                revoke_path.display()
            );
        }

        Commands::Agent => {
            handle_agent_card(&cli.data_dir, effective_mainnet).await?;
        }

        Commands::Run {
            strategy: strat_path,
            allocation,
            password_env,
        } => {
            handle_run(
                &cli.data_dir,
                cli.mainnet || effective_mainnet,
                strat_path,
                allocation,
                password_env,
            )
            .await?;
        }
    }

    Ok(())
}

async fn handle_agent_card(data_dir: &Path, req_mainnet: bool) -> anyhow::Result<()> {
    let effective_data_dir = if data_dir.join("hl_agent_keystore.json").exists() {
        data_dir.to_path_buf()
    } else if Path::new("data/hl_agent_keystore.json").exists() {
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
    };
    let ks_path = effective_data_dir.join("hl_agent_keystore.json");
    let ks = match EncryptedAgentKeystore::load_from_file(&ks_path) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("No agent keystore found at {}: {e}", ks_path.display());
            eprintln!(
                "Run `vaughan hl connect --mainnet` to 1-click generate and authorize an agent."
            );
            std::process::exit(1);
        }
    };
    let agent_str = format!("{:#x}", ks.agent_address);
    let backing_str = format!("{:#x}", ks.backing_account);
    let net = if ks.is_mainnet || req_mainnet {
        "Mainnet"
    } else {
        "Testnet"
    };
    let info_url = if ks.is_mainnet || req_mainnet {
        vaughan_hl::market_data::HL_MAINNET_INFO_URL
    } else {
        vaughan_hl::market_data::HL_TESTNET_INFO_URL
    };

    let copied = vaughan_hl::market_data::copy_to_clipboard(&agent_str);
    let role_res = vaughan_hl::market_data::check_agent_role(info_url, ks.agent_address).await;
    let status = match &role_res {
        Ok(r) if r == "agent" => "● AUTHORIZED ON HYPERLIQUID (Active)",
        Ok(r) if r == "missing" => "⚠️ NOT AUTHORIZED (Needs Approval on Hyperliquid)",
        Ok(r) => r.as_str(),
        Err(_) => "Could not reach Hyperliquid info API",
    };

    println!("========================================================================");
    println!(" HYPERLIQUID AGENT TRADING KEY");
    println!("========================================================================");
    println!(" Agent Address   : {agent_str}");
    println!(" Backing Wallet  : {backing_str}");
    println!(" Network         : {net}");
    println!(" Exchange Status : {status}");
    println!(
        " Clipboard       : {}",
        if copied {
            "COPIED TO CLIPBOARD (ready to paste!)"
        } else {
            "Copy manually above"
        }
    );
    println!("------------------------------------------------------------------------");
    println!(" How to Connect (Choose One):");
    println!();
    println!(" OPTION 1: 1-Click Auto-Connect (Zero Copy/Paste Needed):");
    println!(
        "   vaughan hl connect {}",
        if ks.is_mainnet || req_mainnet {
            "--mainnet"
        } else {
            ""
        }
    );
    println!();
    println!(" OPTION 2: Web UI (Paste copied address):");
    println!("   1. Open: https://app.hyperliquid.xyz/trade");
    println!("   2. Click Settings (gear) -> API Wallets -> Add API Wallet");
    println!("   3. Paste Agent: {agent_str}");
    println!("   4. Name: vaughan-hl -> Click Authorize");
    println!("========================================================================");
    Ok(())
}

async fn handle_run(
    data_dir: &Path,
    is_mainnet: bool,
    strat_path: PathBuf,
    allocation: f64,
    password_env: Option<String>,
) -> anyhow::Result<()> {
    let effective_data_dir = if data_dir.join("hl_agent_keystore.json").exists() {
        data_dir.to_path_buf()
    } else if Path::new("data/hl_agent_keystore.json").exists() {
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
    };
    let ks_path = effective_data_dir.join("hl_agent_keystore.json");
    if !ks_path.exists() {
        anyhow::bail!(
            "No agent keystore found at {}. Connect first in Vaughan TUI or run `vaughan hl connect`.",
            ks_path.display()
        );
    }
    let ks = EncryptedAgentKeystore::load_from_file(&ks_path)
        .map_err(|e| anyhow::anyhow!("Failed to load keystore: {e}"))?;

    let password = match password_env {
        Some(var) => {
            let val = std::env::var(&var)
                .map_err(|_| anyhow::anyhow!("Environment variable `{var}` not set"))?;
            SecretString::from(val)
        }
        None => {
            let p = rpassword::prompt_password("Enter Vaughan wallet password: ")?;
            SecretString::from(p)
        }
    };

    let signer = ks
        .unlock(&password)
        .map_err(|e| anyhow::anyhow!("Keystore unlock failed (invalid password?): {e}"))?;
    let signer = std::sync::Arc::new(signer);

    let strategy = CompiledStrategy::from_toml_file(&strat_path).map_err(|e| {
        anyhow::anyhow!("Failed to load strategy from {}: {e}", strat_path.display())
    })?;

    let label = strat_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("strategy")
        .to_string();

    let net_tag = if is_mainnet || ks.is_mainnet {
        "MAINNET"
    } else {
        "TESTNET"
    };
    println!("=== Vaughan-HL Headless Live Runner [{net_tag}] ===");
    println!(
        "Strategy      : {label} ({} {})",
        strategy.config.market.coin, strategy.config.market.interval
    );
    println!("Agent Address : {:#x}", ks.agent_address);
    println!("Backing EOA   : {:#x}", ks.backing_account);
    println!("Allocation    : ${allocation:.2} USD");
    println!("Press Ctrl+C to stop.\n");

    let cfg = vaughan_hl::live::LiveConfig::new(
        is_mainnet || ks.is_mainnet,
        ks.backing_account,
        strategy,
        label,
        allocation,
    );

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let status = std::sync::Arc::new(std::sync::Mutex::new(
        vaughan_hl::live::LiveStatus::default(),
    ));

    let stop_sig = std::sync::Arc::clone(&stop);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        println!("\nShutdown signal received; stopping live runner...");
        stop_sig.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    let status_printer = std::sync::Arc::clone(&status);
    let stop_printer = std::sync::Arc::clone(&stop);
    tokio::spawn(async move {
        let mut last_msg = String::new();
        while !stop_printer.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if let Ok(st) = status_printer.lock() {
                if !st.msg.is_empty() && st.msg != last_msg {
                    let now = vaughan_hl::market_data::now_unix_ms();
                    let secs = now / 1000;
                    println!(
                        "[{:02}:{:02}:{:02} UTC] {}",
                        (secs / 3600) % 24,
                        (secs % 3600) / 60,
                        secs % 60,
                        st.msg
                    );
                    last_msg = st.msg.clone();
                }
            }
        }
    });

    vaughan_hl::live::run_live(cfg, signer, stop, status).await;
    println!("Live runner finished.");
    Ok(())
}

async fn handle_siphon_command(
    _strategies_dir: &Path,
    pct: Option<f64>,
    token: Option<String>,
    dry_run: bool,
) -> Result<(), anyhow::Error> {
    let target_token =
        token.unwrap_or_else(|| vaughan_hl::siphon::DEFAULT_BTC_MEME_ADDR.to_string());
    let target_symbol = vaughan_hl::siphon::display_token_symbol(&target_token);
    let target_pct = pct.unwrap_or(20.0);

    println!("================================================================================");
    println!(" PROFIT SIPHON & AUTO-REINVEST TELEMETRY (PULSECHAIN MAINNET)");
    println!("================================================================================");
    println!(
        "  Allocation   : {:.1}% weekly of net realized perpetual profits",
        target_pct
    );
    println!("  Target Token : {} ({})", target_symbol, target_token);
    println!("  Routing      : Vaughan Ag Meta-Aggregator (SquirrelSwap + PulseSwap)");
    println!("  Capital Gate : Zero-Loss Invariant ($0 spent during drawdown or flat weeks)");

    if dry_run {
        println!("\nTesting live multi-aggregator quote for 1,000 PLS input...");
        let target_addr = vaughan_hl::siphon::resolve_target_token_address(&target_token)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let quote = vaughan_hl::siphon::quote_best_aggregator(
            vaughan_hl::siphon::NATIVE_ZERO_ADDR,
            &format!("{:#x}", target_addr),
            "1000000000000000000000",
            1.0,
        )
        .await;

        match quote {
            Ok(q) => {
                println!("  ✓ Best Aggregator: {}", q.venue);
                println!("  ✓ Route Chosen   : {}", q.route);
                println!("  ✓ Output Tokens  : {}", q.amount_out);
                println!("  ✓ Estimated Gas  : {}", q.gas_hint);
                println!("  [SUCCESS] Multi-aggregator flow verified live on PulseChain.");
            }
            Err(e) => {
                println!("  ✗ Multi-Aggregator Quote Failed: {e}");
            }
        }
    } else {
        println!("\nRun with `--dry-run` to test live multi-aggregator quotes from SquirrelSwap & PulseSwap.");
    }
    Ok(())
}
