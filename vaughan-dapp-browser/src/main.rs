//! Vaughan optional Chromium dApp browser (Phase 1).
//!
//! Modular side door: allowlisted HTTPS only, injects an EIP-1193 shim that
//! talks to Vaughan's local provider WebSocket. Signing stays in the TUI.
//!
//! Engine for Phase 1 is **system Chromium** plus a tiny unpacked extension
//! (no CEF, no chromiumoxide page automation). Soft-launch seam stays
//! `vaughan-dapp-browser` on `PATH`.

mod allowlist;
mod extension_assets;
mod launch;
mod provider_inject;

use std::process::ExitCode;

use clap::Parser;

use crate::allowlist::Allowlist;
use crate::launch::{run_browser, run_self_check, LaunchOpts};

/// Optional allowlisted Chromium shell for Vaughan (EIP-1193 → local provider).
#[derive(Debug, Parser)]
#[command(name = "vaughan-dapp-browser", version, about)]
struct Cli {
    /// Initial HTTPS URL (must pass the allowlist). Required unless `--self-check`.
    #[arg(long, required_unless_present = "self_check")]
    url: Option<String>,

    /// Open a local page that shows PASS/FAIL for Vaughan inject.
    #[arg(long, default_value_t = false)]
    self_check: bool,

    /// Vaughan EIP-1193 WebSocket URL (loopback).
    #[arg(long, default_value = "ws://127.0.0.1:8745")]
    provider_ws: String,

    /// Allowed host suffix (repeatable). If empty, only the `--url` host is allowed.
    #[arg(long = "allow-host")]
    allow_hosts: Vec<String>,

    /// Export CDP on this port for agent control (`0` = do not advertise).
    #[arg(long, default_value_t = 0)]
    cdp_port: u16,

    /// Chromium / Chrome binary (defaults: chromium, google-chrome, …).
    #[arg(long)]
    chrome: Option<String>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("vaughan-dapp-browser: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let provider_ws = crate::launch::resolve_provider_ws(&cli.provider_ws)?;
    if cli.self_check {
        return run_self_check(&provider_ws, cli.chrome);
    }
    let url = cli.url.ok_or_else(|| "--url is required".to_string())?;
    let allow = Allowlist::from_url_and_hosts(&url, &cli.allow_hosts)?;
    allow
        .check_url(&url)
        .map_err(|e| format!("url not allowlisted: {e}"))?;

    run_browser(LaunchOpts {
        url,
        provider_ws,
        allow,
        cdp_port: cli.cdp_port,
        chrome: cli.chrome,
    })
}
