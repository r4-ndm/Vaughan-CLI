//! Tracing/logging setup shared by the core and the UI.

use std::fs::OpenOptions;
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;

/// Initialise the `tracing` subscriber for non-interactive CLI use (stderr).
///
/// Respects the `RUST_LOG` environment variable and defaults to `info`.
pub fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
}

/// Initialise logging for the interactive TUI.
///
/// Writes to `<data_dir>/vaughan-cli/vaughan.log` so log lines never paint over
/// the ratatui frame. (stderr shares the terminal screen with the TUI — it is
/// not a safe log sink while the UI is running.)
///
/// Respects `RUST_LOG` (default `info`). If the log file cannot be opened,
/// logging is discarded rather than falling back to the terminal.
pub fn init_tui_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    match open_tui_log_file() {
        Ok(file) => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(false)
                .with_ansi(false)
                .with_writer(Mutex::new(file))
                .try_init()
                .ok();
        }
        Err(_) => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(false)
                .with_writer(std::io::sink)
                .try_init()
                .ok();
        }
    }
}

fn open_tui_log_file() -> std::io::Result<std::fs::File> {
    let base = dirs::data_dir()
        .ok_or_else(|| std::io::Error::other("no data directory"))?
        .join("vaughan-cli");
    std::fs::create_dir_all(&base)?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(base.join("vaughan.log"))
}
