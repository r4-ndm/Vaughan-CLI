//! Library target for vaughan-tui.
//!
//! The binary (`main.rs`) and integration tests both consume this crate; the
//! TUI internals that tests need (the provider approval flow) live in
//! [`provider`]. The unified `vaughan` binary calls [`run_interactive`] when
//! launched with no subcommand.

pub mod app;
pub mod brand;
pub mod freedom;
pub mod input;
pub mod jobs;
pub mod provider;
pub mod views;

use std::io;

use app::App;

/// Start the interactive wallet TUI (ratatui event loop).
///
/// Used by the `vaughan-tui` binary and by the unified `vaughan` entry point
/// when no CLI subcommand is given.
pub fn run_interactive() -> io::Result<()> {
    vaughan_core::logging::init_logging();

    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let mut terminal = ratatui::init();
    let result = run_terminal(&mut terminal, runtime.handle().clone());
    ratatui::restore();
    result
}

fn run_terminal(
    terminal: &mut ratatui::DefaultTerminal,
    handle: tokio::runtime::Handle,
) -> io::Result<()> {
    let mut app = App::new(handle).map_err(|e| io::Error::other(e.user_message()))?;
    app.run(terminal)
}
