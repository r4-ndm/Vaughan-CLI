//! Vaughan-CLI terminal wallet: entry point and terminal lifecycle.

fn main() -> std::io::Result<()> {
    // Dev binary: honor `--profile <name>` like the unified `vaughan` CLI.
    let mut args = std::env::args().skip(1);
    let mut profile = vaughan_core::core::DEFAULT_PROFILE.to_string();
    while let Some(arg) = args.next() {
        if arg == "--profile" {
            if let Some(name) = args.next() {
                profile = name;
            }
        }
    }
    vaughan_tui::run_interactive(&profile)
}
