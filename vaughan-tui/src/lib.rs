//! Library target for vaughan-tui.
//!
//! The binary (`main.rs`) and integration tests both consume this crate; the
//! TUI internals that tests need (the provider approval flow) live in
//! [`provider`]. The unified `vaughan` binary calls [`run_interactive`] when
//! launched with no subcommand.

pub mod app;
pub mod brand;
pub mod clipboard;
pub mod dapp_browser;
pub mod freedom;
pub mod input;
pub mod intent;
pub mod jobs;
pub mod mcp;
pub mod provider;
pub mod sentient_mcp;
pub mod views;

use std::io;

use app::App;

/// Start the interactive wallet TUI (ratatui event loop).
///
/// Used by the `vaughan-tui` binary and by the unified `vaughan` entry point
/// when no CLI subcommand is given. `profile` selects the vault profile
/// (`default` = adviser; `sentient` = agent auto-exec) and pre-selects that
/// profile on the unlock-screen picker.
pub fn run_interactive(profile: &str) -> io::Result<()> {
    vaughan_core::logging::init_tui_logging();
    if let Err(e) = vaughan_core::core::reject_deferred_sentient_profile(profile) {
        return Err(io::Error::other(e.user_message()));
    }
    brand::load_persisted_theme();

    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let mut terminal = ratatui::init();
    let result = run_terminal(&mut terminal, runtime.handle().clone(), profile);
    ratatui::restore();
    result
}

fn run_terminal(
    terminal: &mut ratatui::DefaultTerminal,
    handle: tokio::runtime::Handle,
    profile: &str,
) -> io::Result<()> {
    let mut app = App::new(handle, profile).map_err(|e| io::Error::other(e.user_message()))?;
    app.run(terminal)
}
