//! Tracing/logging setup shared by the core and the UI.

use tracing_subscriber::EnvFilter;

/// Initialise the `tracing` subscriber.
///
/// Respects the `RUST_LOG` environment variable and defaults to `info`.
pub fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        // The TUI owns stdout; logs go to stderr so they never corrupt the UI.
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
}
