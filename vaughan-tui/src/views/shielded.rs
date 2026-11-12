//! RAILGUN shielded balances on the Receive screen (opt-in `railgun` feature).
//!
//! Read-only: derives the 0zk account from the active HD account, scans
//! RAILGUN logs on a background thread (resuming from the encrypted cache),
//! and shows per-token balances. No POI calls and no signing. The 0zk receive
//! address is shown on Sepolia only until Vaughan can spend shielded funds.
//! On mainnets, `f` runs one quick-sync from the public RAILGUN indexer
//! (opt-in per run: the indexer sees our IP, not our notes).

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use alloy::primitives::utils::format_units;
use alloy::primitives::Address;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use tokio::runtime::Handle;
use vaughan_core::core::WalletState;
use vaughan_railgun::balance::{balances, token_metadata, ShieldedBalances};
use vaughan_railgun::cache::{self, CacheKey};
use vaughan_railgun::deployment;
use vaughan_railgun::keys::RailgunAccount;
use vaughan_railgun::quick::{self, Indexer};
use vaughan_railgun::scan::{self, ScanOptions, ScanProgress, ScanState};

/// Save the cache at most this often while scanning.
const CHECKPOINT_EVERY: Duration = Duration::from_secs(15);

enum Msg {
    Progress(ScanProgress),
    Balances(ShieldedBalances),
    Token(Address, String, u8),
    Status(String),
    /// Final status line.
    Done(String),
}

/// Shielded balance panel state; owns the background scan.
#[derive(Default)]
pub struct ShieldedPanel {
    address: Option<String>,
    chain_id: u64,
    rx: Option<mpsc::Receiver<Msg>>,
    stop: Arc<AtomicBool>,
    progress: Option<ScanProgress>,
    balances: Option<ShieldedBalances>,
    tokens: HashMap<Address, (String, u8)>,
    status: String,
}

impl Drop for ShieldedPanel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl ShieldedPanel {
    /// True while a background scan is running.
    pub fn is_scanning(&self) -> bool {
        self.rx.is_some()
    }

    /// Start (or resume) a scan for the active account and network.
    pub fn start(&mut self, wallet: &WalletState, handle: &Handle) {
        self.start_scan(wallet, handle, false);
    }

    /// Catch up from the public RAILGUN indexer, then finish over RPC.
    pub fn quick_sync(&mut self, wallet: &WalletState, handle: &Handle) {
        if !quick::is_available(wallet.networks().active().chain_id) {
            self.status = "No RAILGUN indexer for this network — r scans over RPC.".into();
            return;
        }
        self.start_scan(wallet, handle, true);
    }

    fn start_scan(&mut self, wallet: &WalletState, handle: &Handle, quick: bool) {
        if self.is_scanning() {
            return;
        }
        let net = wallet.networks().active();
        self.chain_id = net.chain_id;
        let Some(dep) = deployment::for_chain(net.chain_id) else {
            self.status = format!(
                "RAILGUN isn't deployed on {} — switch to Sepolia (n) to view private balances.",
                net.name
            );
            return;
        };
        let account = match wallet
            .with_hd_seed(RailgunAccount::from_seed)
            .and_then(|r| r)
        {
            Ok(a) => Arc::new(a),
            Err(e) => {
                self.status = e.user_message();
                return;
            }
        };
        let urls = match wallet.active_rpc_url_list() {
            Ok(u) if !u.is_empty() => u,
            Ok(_) => {
                self.status = "No RPC configured for this network.".into();
                return;
            }
            Err(e) => {
                self.status = e.user_message();
                return;
            }
        };
        let Some(profile_dir) = wallet.path().parent().map(PathBuf::from) else {
            self.status = "No profile directory for the RAILGUN cache.".into();
            return;
        };

        if self.address.as_deref() != Some(account.address()) {
            self.balances = None;
            self.progress = None;
        }
        self.address = Some(account.address().to_string());
        self.status = "Preparing scan…".into();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.stop = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&self.stop);
        let handle = handle.clone();
        std::thread::spawn(move || {
            run_scan(
                &handle,
                &dep,
                &account,
                &urls,
                &profile_dir,
                quick,
                &stop,
                &tx,
            );
        });
    }

    /// Ask the background scan to stop after its current chunk.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Drain messages from the background scan (call every UI tick).
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let mut done = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Progress(p) => self.progress = Some(p),
                Msg::Balances(b) => self.balances = Some(b),
                Msg::Token(addr, symbol, decimals) => {
                    self.tokens.insert(addr, (symbol, decimals));
                }
                Msg::Status(s) => self.status = s,
                Msg::Done(status) => {
                    self.status = status;
                    done = true;
                }
            }
        }
        if done {
            self.rx = None;
        }
    }

    /// 0zk address to show/copy, only where receiving is enabled (Sepolia).
    pub fn receive_address(&self) -> Option<&str> {
        if deployment::receive_enabled(self.chain_id) {
            self.address.as_deref()
        } else {
            None
        }
    }

    pub fn lines(&self, net_name: &str) -> Vec<Line<'static>> {
        let dim = Style::default().fg(Color::DarkGray);
        let keys = if !deployment::is_supported(self.chain_id) {
            "Esc back"
        } else if deployment::shield_enabled(self.chain_id) {
            "s shield · u unshield · r rescan · x stop · z copy 0zk · Esc back"
        } else if self.receive_address().is_some() {
            "r rescan · x stop · z copy 0zk · Esc back"
        } else if quick::is_available(self.chain_id) {
            "r rescan · f quick-sync · x stop · Esc back"
        } else {
            "r rescan · x stop · Esc back"
        };
        let mut lines = vec![
            Line::from(format!("Private balance (RAILGUN) · {net_name}")),
            Line::from(keys),
            Line::from(""),
        ];
        if !deployment::is_supported(self.chain_id) {
            lines.push(Line::from(self.status.clone()));
            return lines;
        }
        match self.receive_address() {
            Some(addr) => {
                lines.push(Line::from("0zk receive address (testnet):"));
                lines.push(Line::from(Span::styled(
                    addr.to_string(),
                    Style::default().fg(Color::Cyan),
                )));
            }
            None => lines.push(Line::from(Span::styled(
                "0zk receive address is hidden on mainnets until Vaughan can spend \
                 shielded funds. Spending needs Railway wallet (same recovery phrase) for now.",
                dim,
            ))),
        }
        lines.push(Line::from(""));

        match &self.balances {
            None => lines.push(Line::from("No balance yet.")),
            Some(b) if b.tokens.is_empty() && b.other_notes == 0 => {
                lines.push(Line::from("No shielded tokens found."));
            }
            Some(b) => {
                for t in &b.tokens {
                    let text = match self.tokens.get(&t.token) {
                        Some((symbol, decimals)) => format!(
                            "{} {symbol}",
                            format_units(t.amount, *decimals).unwrap_or_else(|_| "?".into())
                        ),
                        None => format!("{} (raw) {:#x}", t.amount, t.token),
                    };
                    lines.push(Line::from(format!("  {text}")));
                }
                if b.other_notes > 0 {
                    lines.push(Line::from(format!("  + {} NFT note(s)", b.other_notes)));
                }
            }
        }

        lines.push(Line::from(""));
        if let Some(p) = self.progress {
            let total = p.target.saturating_sub(p.start).max(1);
            let done = p.scanned_to.saturating_sub(p.start).min(total);
            let pct = done.saturating_mul(100) / total;
            lines.push(Line::from(format!(
                "Scanned to block {} of {} ({pct}%)",
                p.scanned_to, p.target
            )));
        }
        if !self.status.is_empty() {
            lines.push(Line::from(Span::styled(
                self.status.clone(),
                Style::default().fg(Color::Yellow),
            )));
        }
        if quick::is_available(self.chain_id) {
            lines.push(Line::from(Span::styled(
                "f quick-sync downloads all RAILGUN history from the public indexer \
                 (it sees your IP, not your notes).",
                dim,
            )));
        }
        lines.push(Line::from(Span::styled(
            "Shows what the RPC/indexer returned; spendability (POI) isn't checked yet.",
            dim,
        )));
        lines
    }
}

/// Background thread body: load cache, optionally quick-sync, scan across
/// RPCs, save, label tokens.
#[allow(clippy::too_many_arguments)]
fn run_scan(
    handle: &Handle,
    dep: &deployment::RailgunDeployment,
    account: &RailgunAccount,
    urls: &[String],
    profile_dir: &std::path::Path,
    quick: bool,
    stop: &AtomicBool,
    tx: &mpsc::Sender<Msg>,
) {
    let send = |m: Msg| tx.send(m).is_ok();
    let key = match CacheKey::derive(account) {
        Ok(k) => k,
        Err(e) => {
            send(Msg::Done(e.user_message()));
            return;
        }
    };
    let path = cache::cache_path(profile_dir, dep.chain_id, account.hd_index());
    let mut state = match cache::load(&path, &key, dep, account.hd_index()) {
        Ok(Some(s)) => s,
        Ok(None) => ScanState::new(dep),
        Err(_) => {
            send(Msg::Status(
                "Cache unreadable — rescanning from the start.".into(),
            ));
            ScanState::new(dep)
        }
    };
    send(Msg::Balances(balances(&state)));

    let mut saved_at = Instant::now();
    let mut on_progress = |st: &ScanState, p: ScanProgress| {
        if stop.load(Ordering::Relaxed) || !send(Msg::Progress(p)) {
            return ControlFlow::Break(());
        }
        if saved_at.elapsed() >= CHECKPOINT_EVERY {
            saved_at = Instant::now();
            let _ = cache::save(&path, &key, dep, account.hd_index(), st);
            send(Msg::Balances(balances(st)));
        }
        ControlFlow::Continue(())
    };

    if let Some(indexer) = Indexer::for_chain(dep.chain_id).filter(|_| quick) {
        send(Msg::Status("Quick-sync from the RAILGUN indexer…".into()));
        let result = handle.block_on(quick::quick_sync(
            &indexer,
            indexer.range(),
            dep,
            account,
            &mut state,
            &mut on_progress,
        ));
        if let Err(e) = result {
            send(Msg::Status(format!(
                "Quick-sync failed ({}); scanning over RPC…",
                e.user_message()
            )));
        }
    }
    if !stop.load(Ordering::Relaxed) {
        send(Msg::Status("Scanning RAILGUN logs…".into()));
    }

    let mut last_err = None;
    let mut used = None;
    for url in urls {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let Ok(provider) = scan::connect(url) else {
            continue;
        };
        let result = handle.block_on(scan::scan(
            &provider,
            dep,
            account,
            &mut state,
            ScanOptions::default(),
            &mut on_progress,
        ));
        match result {
            Ok(()) => {
                last_err = None;
                used = Some(provider);
                break;
            }
            Err(e) => last_err = Some(e.user_message()),
        }
    }
    if let Err(e) = cache::save(&path, &key, dep, account.hd_index(), &state) {
        send(Msg::Status(format!(
            "Couldn't save RAILGUN cache: {}",
            e.user_message()
        )));
    }
    let totals = balances(&state);
    if let Some(provider) = &used {
        for t in &totals.tokens {
            if let Some((symbol, decimals)) = handle.block_on(token_metadata(provider, t.token)) {
                send(Msg::Token(t.token, symbol, decimals));
            }
        }
    }
    send(Msg::Balances(totals));
    send(Msg::Done(match last_err {
        Some(e) if !quick && quick::is_available(dep.chain_id) => {
            format!("Scan failed: {e} — f quick-syncs from the indexer instead.")
        }
        Some(e) => format!("Scan failed: {e}"),
        None if stop.load(Ordering::Relaxed) => "Stopped — r resumes.".into(),
        None => "Up to date.".into(),
    }));
}

#[cfg(test)]
mod tests {
    use alloy::primitives::U256;
    use vaughan_railgun::balance::TokenBalance;

    use super::*;

    fn text(panel: &ShieldedPanel) -> String {
        panel
            .lines("net")
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn unsupported_chain_shows_only_the_reason() {
        let mut panel = ShieldedPanel::default();
        panel.chain_id = 369;
        panel.status = "RAILGUN isn't deployed on PulseChain — switch to Sepolia (n) to view private balances.".into();
        let t = text(&panel);
        assert!(t.contains("isn't deployed"));
        assert!(t.contains("switch to Sepolia"));
        assert!(!t.contains("0zk"));
    }

    #[test]
    fn receive_address_is_sepolia_only() {
        let mut panel = ShieldedPanel::default();
        panel.chain_id = 1;
        panel.address = Some("0zk1test".into());
        assert!(panel.receive_address().is_none());
        assert!(!text(&panel).contains("0zk1test"));
        panel.chain_id = deployment::SEPOLIA_CHAIN_ID;
        assert_eq!(panel.receive_address(), Some("0zk1test"));
        assert!(text(&panel).contains("0zk1test"));
    }

    #[test]
    fn formats_known_tokens_and_raw_fallback() {
        let usdc = Address::repeat_byte(1);
        let other = Address::repeat_byte(2);
        let mut panel = ShieldedPanel::default();
        panel.chain_id = deployment::SEPOLIA_CHAIN_ID;
        panel.address = Some("0zk1test".into());
        panel.balances = Some(ShieldedBalances {
            tokens: vec![
                TokenBalance {
                    token: usdc,
                    amount: U256::from(1_500_000),
                    notes: 2,
                },
                TokenBalance {
                    token: other,
                    amount: U256::from(7),
                    notes: 1,
                },
            ],
            other_notes: 1,
        });
        panel.tokens.insert(usdc, ("USDC".into(), 6));
        let t = text(&panel);
        assert!(t.contains("1.500000 USDC"), "{t}");
        assert!(t.contains("7 (raw)"));
        assert!(t.contains("1 NFT note"));
        assert!(t.contains("POI"));
    }
}
