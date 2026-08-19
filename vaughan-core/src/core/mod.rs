//! Core wallet services: persistence, accounts, network, transactions, and the
//! top-level [`WalletState`] that ties them together for the UI.

pub mod account;
pub mod network;
pub mod persistence;
pub mod profile;
pub mod stealth;
pub mod transaction;
pub mod wallet;

pub use account::{Account, AccountManager};
pub use network::NetworkService;
pub use persistence::{PersistedState, StateManager, DEFAULT_PROFILE, DEGEN_PROFILE};
pub use profile::OperatingMode;
pub use stealth::{looks_like_stealth_uri, StealthNote, StealthSendResult};
pub use transaction::{format_base_units, parse_native_amount, TransactionService};
pub use wallet::WalletState;
