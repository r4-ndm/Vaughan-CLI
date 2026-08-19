//! Vaughan-CLI terminal wallet: entry point and terminal lifecycle.

fn main() -> std::io::Result<()> {
    vaughan_tui::run_interactive()
}
