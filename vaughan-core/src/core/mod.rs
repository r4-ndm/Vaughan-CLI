//! Core wallet services: persistence, accounts, network, transactions, and the
//! top-level [`WalletState`] that ties them together for the UI.

pub mod account;
pub mod aggregator;
pub mod network;
pub mod persistence;
pub mod piteas;
pub mod profile;
pub mod stealth;
pub mod transaction;
pub mod vault_secrets;
pub mod wallet;

pub use account::{Account, AccountManager};
pub use aggregator::{
    quote_aggregator, AggAccess, AggExecTx, AggQuote, AggQuoteRequest, AggVenue, AGG_VENUES,
};
pub use network::NetworkService;
pub use persistence::{
    default_trusted_dapps, merge_default_trusted_dapps, CustomNetwork, CustomToken, PersistedState,
    StateManager, TrustedDapp, DEFAULT_PROFILE, DEGEN_PROFILE,
};
pub use piteas::{
    AuthStyle, MethodParameters, NativeToken, PiteasClient, PiteasFileConfig, PiteasQuote,
    QuoteRequest, PITEAS_ROUTER_MAINNET,
};
pub use profile::OperatingMode;
pub use stealth::{looks_like_stealth_uri, StealthNote, StealthSendResult};
pub use transaction::{format_base_units, parse_native_amount, TransactionService};
pub use wallet::{ChromeRpcSnapshot, WalletState};
