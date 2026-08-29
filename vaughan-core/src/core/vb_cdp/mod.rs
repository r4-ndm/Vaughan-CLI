//! CDP WebSocket client for VB agent navigation (MCP B2).
//!
//! Uses `Runtime.evaluate` + `Input.*` over a page WebSocket — same approach as
//! `docs/spikes/cef-tauri` `cdp_ax_smoke`, without chromiumoxide.
//!
//! Split by concern:
//! - [`client`] — page WebSocket session (`CdpPage`) and cross-frame helpers
//! - [`js`] — embedded JS snippets (`include_str!`) + placeholder builders
//! - [`snapshot`] — interactive-element snapshot + visible text lines
//! - [`quote`] — swap quote parsing from visible lines
//! - [`swap`] — token pickers, sell amount, quote CTA, one-shot setup
//! - [`interact`] — click/type/press/wait, modal dismissal, wallet connect

mod client;
mod interact;
mod js;
mod quote;
mod snapshot;
mod swap;

pub use client::{cdp_page_ws_url, ElementRef};
pub use interact::{
    cdp_click, cdp_click_by_text, cdp_connect_vaughan_wallet, cdp_dismiss_modals, cdp_press,
    cdp_type, cdp_wait,
};
pub use quote::{cdp_read_quote, infer_token_out, parse_quote_hints};
pub use snapshot::cdp_snapshot;
pub use swap::{
    cdp_click_swap_submit, cdp_select_swap_token, cdp_set_swap_amount, cdp_setup_swap,
    normalize_swap_symbol, SwapTokenSide,
};
