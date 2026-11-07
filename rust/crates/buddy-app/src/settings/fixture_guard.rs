//! Shared canonical disposable Mac guard, also exercised by native test children.
use std::path::Path;

fn allowed(actions: &str, fixture: &str, config: &Path, root: &Path) -> bool {
    if actions != "true" || fixture != "1" {
        return false;
    }
    match (config.canonicalize(), root.canonicalize()) {
        (Ok(config), Ok(root)) => root.is_dir() && config.is_file() && config.starts_with(&root),
        _ => false,
    }
}

pub(crate) fn enabled() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let get = |key| std::env::var(key).unwrap_or_default();
    allowed(
        &get("GITHUB_ACTIONS"),
        &get("BUDDY_SETTINGS_FIXTURE"),
        Path::new(&get("BUDDY_CONFIG_PATH")),
        Path::new(&get("RUNNER_TEMP")),
    ) && !get("RUNNER_TEMP").is_empty()
        && !get("BUDDY_CONFIG_PATH").is_empty()
}

#[cfg(test)]
mod tests {
    #[test]
    fn both_flags_and_an_existing_owned_config_are_required() {
        let root = std::env::temp_dir().join(format!("buddy-sound-guard-{}", std::process::id()));
        std::fs::create_dir_all(root.join("fixture")).unwrap();
        let config = root.join("fixture/config.json");
        std::fs::write(&config, "{}").unwrap();
        assert!(super::allowed("true", "1", &config, &root.join("fixture")));
        assert!(!super::allowed("false", "1", &config, &root));
        assert!(!super::allowed("true", "0", &config, &root));
        assert!(!super::allowed("true", "1", &root.join("absent"), &root));
        assert!(!super::allowed(
            "true",
            "1",
            &config,
            &root.join("fixture/config.json")
        ));
        std::fs::write(root.join("outside.json"), "{}").unwrap();
        assert!(!super::allowed(
            "true",
            "1",
            &root.join("outside.json"),
            &root.join("fixture")
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_mac_is_disabled() {
        assert!(!super::enabled());
    }
}
