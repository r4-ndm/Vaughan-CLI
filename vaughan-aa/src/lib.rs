//! Vaughan AA: Ambire smart-account (ERC-4337/7702 AA) support, reimplemented
//! from the on-chain `AmbireAccount` contract.
//!
//! This crate carries over only the *interface facts* of Ambire's deployed,
//! verified smart account — the ABI selectors, struct shapes, and the
//! digest/signature layout — and writes every line of encoding, hashing, and
//! signing fresh. The contract itself (AGPL-3.0) is never copied or
//! reimplemented; we only `call` it. See `docs/ambire-aa.md` and the provenance
//! note in `README.md`.
//!
//! `Vaughan-Dioxus` is a reference guide only, never a code source (CLAUDE.md).

pub mod abi;
pub mod adapter;
pub mod build;
pub mod encode;
pub mod scw;
pub mod sign;

pub use abi::Transaction;
pub use adapter::{
    bootstrap_delegation, estimate_self_pay_fee, get_account_nonce, is_delegated, submit_batch,
    submit_self_pay, BatchSubmitResult, AMBIRE_IMPLEMENTATION,
};
pub use encode::{decode_execute, encode_execute};
