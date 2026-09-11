//! EVM chain adapter (Alloy-backed).

pub mod abi;
pub mod adapter;
pub mod networks;
pub mod tokens;
pub mod utils;

pub use adapter::EvmAdapter;
pub use networks::{
    explorer_address_url, get_network_by_chain_id, get_network_by_id, EvmNetworkConfig,
};
pub use tokens::{find_token, tokens_for_chain, TokenEntry};
