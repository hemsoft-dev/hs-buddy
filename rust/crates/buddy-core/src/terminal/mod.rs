//! Terminal emulation and owned interactive sessions, independent of the desktop renderer.

mod host;
mod input;
mod output;
mod ownership;
mod protocol;
mod screen;
mod session;
mod shell;

pub use host::run_terminal_host;
pub use input::{
    InputBudget, InputError, Key, MAX_PASTE_BYTES, Modifiers, encode_key, encode_paste,
};
pub use screen::{CellColor, CellStyle, ScreenCell, ScreenSnapshot, TerminalScreen, TerminalSize};
pub use session::{InputStatus, InputTicket, Session, SessionSnapshot, SessionStatus};
pub use shell::ShellSpec;
