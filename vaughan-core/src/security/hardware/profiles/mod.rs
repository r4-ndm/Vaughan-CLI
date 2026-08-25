//! EVM signing profile for hardware / local backends.

pub mod evm;

pub use evm::{
    default_evm_derivation_path, sign_evm_local, sign_evm_typed_data_local, sign_prepared_evm_tx,
};
