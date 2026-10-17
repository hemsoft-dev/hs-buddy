//! Candidate locations only. Discovery does not inspect the filesystem or read a profile.
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Mac,
    Linux,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub name: &'static str,
    pub user_directory: PathBuf,
}

/// Inputs are supplied by the application, keeping environment access and
/// directory selection separate from the confined read service. Portable or
/// --user-data-dir installations use an explicitly selected User directory.
pub fn standard_sources(
    platform: Platform,
    home: Option<&Path>,
    app_data: Option<&Path>,
    xdg_config_home: Option<&Path>,
) -> Vec<Source> {
    let nonempty = |path: &&Path| !path.as_os_str().is_empty() && path.is_absolute();
    let base = match platform {
        Platform::Windows => app_data.filter(nonempty).map(Path::to_path_buf),
        Platform::Mac => home
            .filter(nonempty)
            .map(|path| path.join("Library/Application Support")),
        Platform::Linux => xdg_config_home
            .filter(nonempty)
            .map(Path::to_path_buf)
            .or_else(|| home.filter(nonempty).map(|path| path.join(".config"))),
    };
    let Some(base) = base else { return Vec::new() };
    [("VS Code", "Code"), ("VS Code Insiders", "Code - Insiders")]
        .into_iter()
        .map(|(name, folder)| Source {
            name,
            user_directory: base.join(folder).join("User"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        std::env::temp_dir().join("synthetic-home-界")
    }

    #[test]
    fn windows_requires_appdata_and_lists_both_editions() {
        let home = home();
        assert!(standard_sources(Platform::Windows, Some(&home), None, None).is_empty());
        let sources = standard_sources(Platform::Windows, None, Some(&home), None);
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].user_directory, home.join("Code/User"));
        assert_eq!(sources[1].user_directory, home.join("Code - Insiders/User"));
    }

    #[test]
    fn mac_uses_application_support_and_linux_respects_absolute_xdg() {
        let home = home();
        let xdg = home.join("separate-config");
        let mac = standard_sources(Platform::Mac, Some(&home), None, Some(&xdg));
        assert_eq!(
            mac[0].user_directory,
            home.join("Library/Application Support/Code/User")
        );
        let linux = standard_sources(Platform::Linux, Some(&home), None, Some(&xdg));
        assert_eq!(linux[0].user_directory, xdg.join("Code/User"));
        let fallback = standard_sources(
            Platform::Linux,
            Some(&home),
            None,
            Some(Path::new("relative")),
        );
        assert_eq!(fallback[0].user_directory, home.join(".config/Code/User"));
        assert!(standard_sources(Platform::Linux, None, None, None).is_empty());
    }

    #[test]
    fn relative_and_empty_roots_never_turn_into_current_directory_scans() {
        for path in [Path::new(""), Path::new("relative")] {
            assert!(standard_sources(Platform::Windows, None, Some(path), None).is_empty());
            assert!(standard_sources(Platform::Mac, Some(path), None, None).is_empty());
            assert!(standard_sources(Platform::Linux, Some(path), None, Some(path)).is_empty());
        }
    }
}
