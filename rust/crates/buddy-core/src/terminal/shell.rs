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

    /// [`ShellSpec::configured`], except that a saved directory which no longer
    /// exists, or which this user cannot enter, starts the shell in the home
    /// directory, as Electron's
    /// `parseValidCwd` does. The unavailable directory is returned so the
    /// caller can say where the shell started and why; the fallback is never
    /// silent.
    pub fn configured_or_home(
        settings: Option<&Value>,
        directory: &Path,
    ) -> Result<(Self, Option<PathBuf>), &'static str> {
        let unavailable = !directory.as_os_str().is_empty() && !enterable_directory(directory);
        if unavailable {
            let spec = Self::configured(settings, Path::new(""))?;
            return Ok((spec, Some(directory.to_path_buf())));
        }
        Self::configured(settings, directory).map(|spec| (spec, None))
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

/// UTF-8 console encoding, then Electron's `buildPowerShellOsc7Setup` prompt
/// hook: each prompt reports `$PWD` as OSC 7, so the tab and pane follow `cd`.
/// An absolute directory this user may list and enter. Electron's
/// `inspectCwdAccess` asks for read access; entering also needs search
/// (execute) permission on Unix, or the shell cannot start there.
fn enterable_directory(directory: &Path) -> bool {
    if !directory.is_absolute() || !directory.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let Ok(path) = std::ffi::CString::new(directory.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `path` is a valid NUL-terminated C string for the call.
        unsafe { libc::access(path.as_ptr(), libc::R_OK | libc::X_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        std::fs::read_dir(directory).is_ok()
    }
}

#[cfg(windows)]
fn default_arguments(_: &Path) -> Vec<String> {
    vec![
        "-NoLogo".into(),
        "-NoExit".into(),
        "-Command".into(),
        format!(
            "[Console]::InputEncoding=[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false);{POWERSHELL_OSC7_PROMPT}"
        ),
    ]
}

/// Wraps the existing prompt so a user profile's prompt still runs.
#[cfg_attr(not(windows), allow(dead_code))]
const POWERSHELL_OSC7_PROMPT: &str = r#"$__hsb_op=$function:prompt;function global:prompt{$e=[char]0x1b;$u=(($PWD.ProviderPath -replace '\\','/') -split '/' | ForEach-Object {if($_ -match '^[A-Za-z]:$'){$_}else{[Uri]::EscapeDataString($_)}}) -join '/';[Console]::Write("$e]7;file:///$u$e\");& $__hsb_op}"#;

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

/// `$SHELL`, else Electron's `/bin/bash`, else `/bin/sh` where bash is absent.
#[cfg(unix)]
fn default_program() -> PathBuf {
    std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if Path::new("/bin/bash").is_file() {
                "/bin/bash".into()
            } else {
                "/bin/sh".into()
            }
        })
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
    fn an_unavailable_saved_directory_starts_in_home_and_is_reported() {
        let cwd = std::env::temp_dir();
        let missing = cwd.join(format!("buddy-gone-directory-{}", std::process::id()));
        assert!(!missing.exists());
        let (spec, unavailable) = ShellSpec::configured_or_home(None, &missing).unwrap();
        assert_eq!(unavailable.as_deref(), Some(missing.as_path()));
        assert_eq!(spec.directory, default_directory().unwrap());
        let (spec, unavailable) = ShellSpec::configured_or_home(None, &cwd).unwrap();
        assert_eq!((spec.directory, unavailable), (cwd.clone(), None));
        let (_, unavailable) = ShellSpec::configured_or_home(None, Path::new("")).unwrap();
        assert_eq!(unavailable, None);
        let relative = Path::new("relative/folder");
        let (_, unavailable) = ShellSpec::configured_or_home(None, relative).unwrap();
        assert_eq!(unavailable.as_deref(), Some(relative));
        // Bad shell settings still fail instead of starting another shell.
        let malformed = json!({std::env::consts::OS:{"program":"shell","arguments":"--bad"}});
        assert!(ShellSpec::configured_or_home(Some(&malformed), &missing).is_err());
    }

    #[test]
    fn the_powershell_prompt_hook_reports_the_directory_as_osc_7() {
        // Electron's `buildPowerShellOsc7Setup()` hook, with each path segment
        // percent-encoded (except the drive) so `#`, `%` and spaces survive the
        // file URL; the prompt that was there before still runs.
        assert!(POWERSHELL_OSC7_PROMPT.starts_with("$__hsb_op=$function:prompt;"));
        assert!(POWERSHELL_OSC7_PROMPT.contains("[Uri]::EscapeDataString($_)"));
        assert!(POWERSHELL_OSC7_PROMPT.contains(r#"[Console]::Write("$e]7;file:///$u$e\")"#));
        assert!(POWERSHELL_OSC7_PROMPT.ends_with("& $__hsb_op}"));
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_the_user_cannot_enter_also_starts_in_home() {
        use std::os::unix::fs::PermissionsExt as _;
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return; // root may enter any directory
        }
        let locked = tempfile::tempdir().unwrap();
        std::fs::set_permissions(locked.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = ShellSpec::configured_or_home(None, locked.path());
        std::fs::set_permissions(locked.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let (spec, unavailable) = result.unwrap();
        assert_eq!(unavailable.as_deref(), Some(locked.path()));
        assert_eq!(spec.directory, default_directory().unwrap());
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
