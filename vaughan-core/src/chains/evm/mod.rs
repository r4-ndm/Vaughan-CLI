//! EVM chain adapter (Alloy-backed).

pub mod adapter;
pub mod networks;
pub mod utils;

pub use adapter::EvmAdapter;
pub use networks::EvmNetworkConfig;
