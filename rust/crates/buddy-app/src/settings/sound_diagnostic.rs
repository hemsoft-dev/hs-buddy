//! Disposable hosted Mac fixture observer. Never changes input or configuration.
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

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

fn line(
    out: &mut impl Write,
    phase: &str,
    current: bool,
    requested: Option<bool>,
    saved: Option<bool>,
) {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let json = |value: Option<bool>| match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "null",
    };
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    // Phase is a source literal, never an account/input/error string.
    let _ = writeln!(
        out,
        "buddy-sound-boundary {{\"sequence\":{},\"millis\":{},\"phase\":\"{}\",\"current\":{},\"requested\":{},\"saved\":{}}}",
        SEQUENCE.fetch_add(1, Ordering::Relaxed),
        millis,
        phase,
        current,
        json(requested),
        json(saved)
    );
}

pub(crate) fn record(
    phase: &'static str,
    current: bool,
    requested: Option<bool>,
    saved: Option<bool>,
) {
    if enabled() {
        line(
            &mut std::io::stderr().lock(),
            phase,
            current,
            requested,
            saved,
        );
    }
}

pub(crate) fn mouse(phase: &'static str, x: f32, y: f32, current: bool) {
    if enabled() {
        let _ = writeln!(
            std::io::stderr().lock(),
            "buddy-sound-mouse {{\"phase\":\"{}\",\"x\":{},\"y\":{},\"current\":{}}}",
            phase,
            x,
            y,
            current
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_flags_and_an_existing_owned_config_are_required() {
        let root = std::env::temp_dir().join(format!("buddy-sound-guard-{}", std::process::id()));
        std::fs::create_dir_all(root.join("fixture")).unwrap();
        let config = root.join("fixture/config.json");
        std::fs::write(&config, "{}").unwrap();
        assert!(allowed("true", "1", &config, &root.join("fixture")));
        assert!(!allowed("false", "1", &config, &root));
        assert!(!allowed("true", "0", &config, &root));
        assert!(!allowed("true", "1", &root.join("absent"), &root));
        assert!(!allowed(
            "true",
            "1",
            &config,
            &root.join("fixture/config.json")
        ));
        std::fs::write(root.join("outside.json"), "{}").unwrap();
        assert!(!allowed(
            "true",
            "1",
            &root.join("outside.json"),
            &root.join("fixture")
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn observer_distinguishes_callback_save_failure_and_later_reset() {
        let mut out = Vec::new();
        line(&mut out, "callback", false, Some(true), None);
        line(&mut out, "local-result", false, Some(true), Some(false));
        line(&mut out, "local-result", true, Some(true), Some(true));
        line(&mut out, "replace", false, None, None);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(
            "\"phase\":\"callback\",\"current\":false,\"requested\":true,\"saved\":null"
        ));
        assert!(text.contains("\"current\":false,\"requested\":true,\"saved\":false"));
        assert!(text.contains("\"current\":true,\"requested\":true,\"saved\":true"));
        assert!(text.contains("\"phase\":\"replace\",\"current\":false"));
        assert_eq!(text.lines().count(), 4);
    }
}
