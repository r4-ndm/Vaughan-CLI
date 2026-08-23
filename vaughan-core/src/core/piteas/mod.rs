//! Piteas DEX aggregator client (PulseChain Pathfinder quotes).
//!
//! Public SDK beta: `GET https://sdk.piteas.io/quote` (no key required today,
//! rate-limited). Partner access may add an API key — store it encrypted next
//! to the wallet and attach via configurable [`AuthStyle`].
//!
//! Docs: <https://docs.piteas.io/piteas-sdk-api> · Vaughan: `docs/piteas.md`.

mod client;
mod config;
mod types;

pub use client::{PiteasClient, DEFAULT_QUOTE_PATH};
pub use config::{
    clear_api_key, load_api_key, load_file_config, save_api_key, save_file_config, AuthStyle,
    PiteasFileConfig, PITEAS_KEY_FILE, PITEAS_TOML,
};
pub use types::{
    MethodParameters, NativeToken, PiteasQuote, PiteasToken, QuoteRequest, PITEAS_ROUTER_MAINNET,
};
