//! `vaughan-dapp-browser` — optional allowlisted Chromium dApp shell.
//!
//! ## Phase 1 engine
//!
//! System Chromium + temporary unpacked MV3 extension:
//!
//! - **MAIN** `inject.js` — EIP-1193 / EIP-6963 on `window.ethereum`
//! - **ISOLATED** `content_bridge.js` — `postMessage` ↔ extension port
//! - **background** — owns `WebSocket` to Vaughan (**CSP-safe**; needed for
//!   sites like 9inch that block page-level `ws://`)
//!
//! Signing stays in the Vaughan TUI. Window stays open until you close it.
//!
//! **Known gap:** the host allowlist applies to the *initial* `--url` only;
//! in-tab navigation is not gated yet.
//!
//! ## Usage
//!
//! ```bash
//! # Unlock Vaughan first (provider ws://127.0.0.1:8745)
//! cargo run -p vaughan-cli
//!
//! # Inject + bridge self-check
//! cargo run -p vaughan-dapp-browser -- --self-check
//!
//! # Open a dApp
//! cargo run -p vaughan-dapp-browser -- --url https://app.9inch.io/swap?chain=pulse
//! cargo run -p vaughan-dapp-browser -- --url https://app.pulsex.com/   # then pick an IPFS mirror
//!
//! # Agent CDP export (default off)
//! cargo run -p vaughan-dapp-browser -- --url https://example.com/ --cdp-port 9333
//! ```
//!
//! Flags: `--provider-ws` (loopback only), `--allow-host`, `--chrome`,
//! `--cdp-port`, `--self-check`.
//!
//! Env (TUI soft-launch): `VAUGHAN_DAPP_BROWSER_CMD`,
//! `VAUGHAN_DAPP_BROWSER_CDP_PORT`.
//!
//! TUI `w` → Enter prefers this binary when present, else Freedom.
//!
//! Per-site connect quirks: `vaughan-agent/skills/dapp-connect/`.
//! Strategy: `docs/dapp-browser-strategy.md`.
