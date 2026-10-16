//! Commands that must work without opening a window or reading user settings.

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    Help,
    Version,
    TerminalHost,
    PrepareCopilotRuntime(PathBuf),
    QualifyCopilot(PathBuf),
}

pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Command, &'static str> {
    let mut arguments = arguments.into_iter();
    let Some(argument) = arguments.next() else {
        return Ok(Command::Open);
    };
    if argument == "--prepare-copilot-runtime" || argument == "--qualify-copilot" {
        let root = arguments
            .next()
            .map(PathBuf::from)
            .ok_or("A private runtime directory is required.")?;
        if !root.is_absolute() || arguments.next().is_some() {
            return Err("A single absolute private runtime directory is required.");
        }
        return Ok(if argument == "--qualify-copilot" {
            Command::QualifyCopilot(root)
        } else {
            Command::PrepareCopilotRuntime(root)
        });
    }
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
    fn private_runtime_helper_requires_one_absolute_path_without_echoing_it() {
        let root = std::env::temp_dir().join("synthetic-runtime-界");
        assert_eq!(
            parse([
                "--prepare-copilot-runtime".into(),
                root.clone().into_os_string()
            ]),
            Ok(Command::PrepareCopilotRuntime(root))
        );
        for arguments in [
            vec!["--prepare-copilot-runtime".into()],
            vec![
                "--prepare-copilot-runtime".into(),
                "private-relative".into(),
            ],
            vec![
                "--prepare-copilot-runtime".into(),
                std::env::temp_dir().into_os_string(),
                "private-extra".into(),
            ],
        ] {
            let error = parse(arguments).unwrap_err();
            assert!(!error.contains("private-relative") && !error.contains("private-extra"));
        }
    }

    #[test]
    fn offline_qualification_requires_a_single_absolute_root_before_runtime_guard() {
        let root = std::env::temp_dir().join("synthetic-qualification-界");
        assert_eq!(
            parse(["--qualify-copilot".into(), root.clone().into_os_string()]),
            Ok(Command::QualifyCopilot(root))
        );
        for arguments in [
            vec!["--qualify-copilot".into()],
            vec!["--qualify-copilot".into(), "private-relative".into()],
            vec![
                "--qualify-copilot".into(),
                std::env::temp_dir().into_os_string(),
                "private-extra".into(),
            ],
        ] {
            let error = parse(arguments).unwrap_err();
            assert!(!error.contains("private-relative") && !error.contains("private-extra"));
        }
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
