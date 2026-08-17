//! Vaughan-CLI terminal wallet: entry point and terminal lifecycle.

mod app;
mod input;
mod views;

use std::io;

use app::App;

fn main() -> io::Result<()> {
    vaughan_core::logging::init_logging();

    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, runtime.handle().clone());
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, handle: tokio::runtime::Handle) -> io::Result<()> {
    let mut app = App::new(handle).map_err(|e| io::Error::other(e.user_message()))?;
    app.run(terminal)
}
