//! Per-intent execution handlers for [`super::Signer`].
//!
//! Single purpose: declare the `ops` intent submodules (`transfer`, `swap`,
//! `approve`, `wrap`, `batch_transfer`, `dca`, `sweep`). Must NOT contain
//! handler bodies, policy mutation logic, or Unix socket server code.

mod approve;
mod batch_transfer;
mod dca;
mod swap;
mod sweep;
mod transfer;
mod wrap;
