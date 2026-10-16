//! Standalone transport entry point for headless PTY qualification.
fn main() {
    if std::env::args().nth(1).as_deref() != Some("--terminal-host") {
        std::process::exit(2);
    }
    std::process::exit(buddy_core::terminal::run_terminal_host());
}
