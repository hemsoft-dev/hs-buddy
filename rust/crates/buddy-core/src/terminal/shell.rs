use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ShellSpec {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub directory: PathBuf,
}

impl ShellSpec {
    pub fn validate(&self) -> Result<(), &'static str> {
        let program = self
            .program
            .to_str()
            .ok_or("The shell executable path must be valid Unicode.")?;
        let cwd = self
            .directory
            .to_str()
            .ok_or("The working directory must be valid Unicode.")?;
        if program.is_empty() || program.len() > 4096 || program.contains('\0') {
            return Err("Choose a shell executable with a valid path.");
        }
        if cwd.is_empty()
            || cwd.len() > 4096
            || cwd.contains('\0')
            || !self.directory.is_absolute()
            || !self.directory.is_dir()
        {
            return Err(
                "The terminal working directory is unavailable. Choose an existing absolute directory.",
            );
        }
        if self.arguments.len() > 64
            || self.arguments.iter().map(String::len).sum::<usize>() > 64 * 1024
            || self.arguments.iter().any(|arg| arg.contains('\0'))
        {
            return Err("The shell argument list exceeds its limits or contains NUL.");
        }
        Ok(())
    }

    /// The current OS owns one entry; arguments are always an array, never a command string.
    pub fn configured(settings: Option<&Value>, directory: &Path) -> Result<Self, &'static str> {
        if settings.is_some_and(|value| !value.is_object()) {
            return Err("Platform shell settings must be a JSON object.");
        }
        let selected = settings.and_then(|value| value.get(std::env::consts::OS));
        let directory = if directory.as_os_str().is_empty() {
            default_directory()?
        } else {
            directory.to_path_buf()
        };
        let spec = match selected {
            Some(value) => {
                let object = value
                    .as_object()
                    .ok_or("The current platform shell setting must be an object.")?;
                let program = object
                    .get("program")
                    .and_then(Value::as_str)
                    .ok_or("The shell setting needs an executable path.")?;
                let arguments = object
                    .get("arguments")
                    .and_then(Value::as_array)
                    .ok_or("The shell arguments must be a JSON array of strings.")?
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(String::from)
                            .ok_or("Every shell argument must be a string.")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Self {
                    program: program.into(),
                    arguments,
                    directory,
                }
            }
            None => {
                let program = default_program();
                let arguments = default_arguments(&program);
                Self {
                    program,
                    arguments,
                    directory,
                }
            }
        };
        spec.validate()?;
        Ok(spec)
    }
}

#[cfg(windows)]
fn default_arguments(_: &Path) -> Vec<String> {
    ["-NoLogo","-NoExit","-Command","[Console]::InputEncoding=[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false)"].into_iter().map(String::from).collect()
}

#[cfg(unix)]
fn default_arguments(_: &Path) -> Vec<String> {
    Vec::new()
}

fn default_directory() -> Result<PathBuf, &'static str> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let path = std::env::var_os(key)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .ok_or("Choose an existing working directory.")?;
    if !path.is_absolute() || !path.is_dir() {
        return Err("Choose an existing working directory.");
    }
    Ok(path)
}

#[cfg(unix)]
fn default_program() -> PathBuf {
    std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| "/bin/sh".into())
}

#[cfg(windows)]
fn default_program() -> PathBuf {
    for name in ["pwsh.exe", "powershell.exe"] {
        if let Some(path) = std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join(name))
                .find(|path| path.is_file())
        }) {
            return path;
        }
    }
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| "C:/Windows".into())
        .join("System32/WindowsPowerShell/v1.0/powershell.exe")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shell_paths_and_arguments_are_preserved_without_command_construction() {
        let cwd = std::env::temp_dir();
        let settings = json!({std::env::consts::OS:{"program":"C:/Program Files/工具/shell.exe","arguments":["--flag","value with spaces","$(not-an-invocation)"]},"other-platform":{"unchanged":true}});
        let spec = ShellSpec::configured(Some(&settings), &cwd).unwrap();
        assert_eq!(
            spec.program,
            PathBuf::from("C:/Program Files/工具/shell.exe")
        );
        assert_eq!(
            spec.arguments,
            vec!["--flag", "value with spaces", "$(not-an-invocation)"]
        );
        assert_eq!(spec.directory, cwd);
    }

    #[test]
    fn invalid_configuration_or_missing_directory_never_silently_falls_back() {
        let cwd = std::env::temp_dir();
        let malformed =
            json!({std::env::consts::OS:{"program":"shell","arguments":"--bad command"}});
        assert!(ShellSpec::configured(Some(&malformed), &cwd).is_err());
        let missing = cwd.join(format!("buddy-missing-directory-{}", std::process::id()));
        assert!(!missing.exists());
        assert!(ShellSpec::configured(None, &missing).is_err());
    }
}
