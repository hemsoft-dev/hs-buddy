//! Commands that must work without opening a window or reading user settings.

use std::ffi::OsString;

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    Help,
    Version,
    TerminalHost,
}

pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Command, &'static str> {
    let mut arguments = arguments.into_iter();
    let Some(argument) = arguments.next() else {
        return Ok(Command::Open);
    };
    if arguments.next().is_some() {
        return Err("Unexpected arguments. Use --help for usage.");
    }
    match argument.to_str() {
        Some("--help" | "-h") => Ok(Command::Help),
        Some("--version" | "-V") => Ok(Command::Version),
        Some("--terminal-host") => Ok(Command::TerminalHost),
        _ => Err("Unknown argument. Use --help for usage."),
    }
}

pub fn print_help() {
    println!(
        "{}\nUsage: {} [--help | --version]\n\nConfiguration:\n  BUDDY_CONVEX_URL   Convex backend URL\n  BUDDY_CONFIG_PATH  Optional isolated settings file",
        env!("BUDDY_PRODUCT_NAME"),
        env!("BUDDY_BINARY_NAME")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_command_starts_the_desktop() {
        assert_eq!(parse([]), Ok(Command::Open));
    }

    #[test]
    fn informational_commands_do_not_start_the_desktop() {
        for argument in ["--help", "-h"] {
            assert_eq!(parse([argument.into()]), Ok(Command::Help));
        }
        for argument in ["--version", "-V"] {
            assert_eq!(parse([argument.into()]), Ok(Command::Version));
        }
    }

    #[test]
    fn private_terminal_host_is_a_separate_non_gui_command() {
        assert_eq!(parse(["--terminal-host".into()]), Ok(Command::TerminalHost));
        assert!(parse(["--terminal-host".into(), "unexpected".into()]).is_err());
    }

    #[test]
    fn unexpected_arguments_do_not_start_the_desktop_or_echo_values() {
        for arguments in [
            vec!["private-value".into()],
            vec!["--version".into(), "private-value".into()],
        ] {
            let error = parse(arguments).unwrap_err();
            assert!(!error.contains("private-value"));
        }
    }
}
