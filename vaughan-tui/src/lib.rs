//! Library target for vaughan-tui.
//!
//! The binary (`main.rs`) and integration tests both consume this crate; the
//! TUI internals that tests need (the provider approval flow) live in
//! [`provider`].

pub mod app;
pub mod input;
pub mod provider;
pub mod views;
