//! Cutover evidence uses only synthetic profiles. Never resolves the real config
//! path or calls an OS vault. Run with `cargo test -p buddy-core --test config_migration`.
use std::{path::Path, process::Command, time::Duration};

use buddy_core::{
    application_settings::LocalEdit,
    config::{AppConfig, ThemeName},
    config_lock::ConfigLock,
    settings_backend::{AccountRecord, SettingsSnapshot},
};
use serde_json::{Value, json};

const ELECTRON: &str = include_str!("fixtures/electron-config.json");

fn update(path: &Path, edit: impl FnOnce(&mut AppConfig)) {
    let held = ConfigLock::acquire(path, Duration::from_secs(10))
        .unwrap()
        .expect("writer must acquire the shared lock");
    let (mut config, stamp) = AppConfig::load_with_stamp_from(path).unwrap();
    edit(&mut config);
    assert!(held.is_held());
    assert!(config.save_if_unchanged_at(path, &stamp).unwrap());
}

// Every supplied Electron value must survive. Native may add defaults.
fn contains(original: &Value, saved: &Value) {
    match original {
        // Electron writes integral dimensions as integers; Rust's f64 fields
        // serialize them with `.0`. Both represent the same JSON number.
        Value::Number(number) => {
            assert_eq!(number.as_f64(), saved.as_f64());
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                contains(value, saved.get(key).expect(key));
            }
        }
        Value::Array(values) => {
            assert_eq!(values.len(), saved.as_array().unwrap().len());
            for (index, value) in values.iter().enumerate() {
                contains(value, &saved[index]);
            }
        }
        _ => assert_eq!(original, saved),
    }
}

#[test]
fn fresh_install_and_restart_are_stable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    assert_eq!(AppConfig::load_from(&path).unwrap(), AppConfig::default());
    assert!(!path.exists(), "loading defaults must be read-only");
    update(&path, |_| {});
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(AppConfig::load_from(&path).unwrap(), AppConfig::default());
    update(&path, |_| {});
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn electron_upgrade_native_edit_and_restart_preserve_all_fixture_values() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(&path, ELECTRON).unwrap();
    let mut expected: Value = serde_json::from_str(ELECTRON).unwrap();
    update(&path, |config| {
        LocalEdit::DashboardCard {
            id: "weather".into(),
            visible: false,
        }
        .apply(config)
        .unwrap();
    });
    expected["ui"]["dashboardCards"]["weather"] = json!(false);
    let bytes = std::fs::read(&path).unwrap();
    contains(&expected, &serde_json::from_slice(&bytes).unwrap());
    let reloaded = AppConfig::load_from(&path).unwrap();
    assert_eq!(reloaded.ui.theme, ThemeName::Light);
    assert_eq!(reloaded.github.accounts.len(), 1);
    // Electron ciphertext remains available for rollback; native deliberately
    // does not interpret it as a remembered location.
    assert!(reloaded.ui.weather_location.is_none());
    update(&path, |_| {});
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn convex_mirror_keeps_local_unknowns_and_does_not_rewrite_other_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(&path, ELECTRON).unwrap();
    let config = AppConfig::load_from(&path).unwrap();
    let mut snapshot = SettingsSnapshot {
        accounts: config
            .github
            .accounts
            .iter()
            .map(AccountRecord::cached)
            .collect(),
        pr: config.pr.clone(),
        copilot: config.copilot.clone(),
    };
    snapshot.accounts[0].repo_root = Some("/fixture/convex-repos".into());
    snapshot.pr.refresh_interval = 60.0;
    snapshot.copilot.model = "backend-model".into();
    update(&path, |config| snapshot.apply_mirror(config).unwrap());
    let mut expected: Value = serde_json::from_str(ELECTRON).unwrap();
    expected["github"]["accounts"][0]["repoRoot"] = json!("/fixture/convex-repos");
    expected["pr"]["refreshInterval"] = json!(60);
    expected["copilot"]["model"] = json!("backend-model");
    contains(
        &expected,
        &serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap(),
    );
    // This tests local reconciliation only, not a live Convex deployment.
}

#[test]
fn stale_writer_is_rejected_without_losing_electron_fields() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(&path, ELECTRON).unwrap();
    let (mut stale, stamp) = AppConfig::load_with_stamp_from(&path).unwrap();
    update(&path, |config| {
        config
            .extra
            .insert("electronNewKey".into(), json!([1, 2, 3]));
    });
    let bytes = std::fs::read(&path).unwrap();
    stale.ui.sidebar_width = 999.0;
    assert!(!stale.save_if_unchanged_at(&path, &stamp).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn writer_process() {
    let Some(path) = std::env::var_os("BUDDY_MIGRATION_CHILD_PATH") else {
        return;
    };
    let path = Path::new(&path);
    let id = std::env::var("BUDDY_MIGRATION_CHILD_ID").unwrap();
    std::fs::write(path.with_extension(format!("ready-{id}")), b"").unwrap();
    let started = std::time::Instant::now();
    while !path.with_extension("go").exists() {
        assert!(started.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(5));
    }
    for iteration in 0..25 {
        update(path, |config| {
            config
                .extra
                .insert(format!("writer-{id}-{iteration}"), json!(iteration));
            // Force overlapping read/modify/write attempts in independent processes.
            std::thread::sleep(Duration::from_millis(2));
        });
    }
}

#[test]
fn concurrent_native_processes_preserve_every_key() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(&path, ELECTRON).unwrap();
    let mut children: Vec<_> = (0..2)
        .map(|id| {
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "writer_process", "--nocapture"])
                .env("BUDDY_MIGRATION_CHILD_PATH", &path)
                .env("BUDDY_MIGRATION_CHILD_ID", id.to_string())
                .spawn()
                .unwrap()
        })
        .collect();
    let started = std::time::Instant::now();
    while !(0..2).all(|id| path.with_extension(format!("ready-{id}")).exists()) {
        assert!(started.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(5));
    }
    std::fs::write(path.with_extension("go"), b"").unwrap();
    for child in &mut children {
        assert!(child.wait().unwrap().success());
    }
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    contains(&serde_json::from_str(ELECTRON).unwrap(), &saved);
    for id in 0..2 {
        for iteration in 0..25 {
            assert_eq!(saved[format!("writer-{id}-{iteration}")], iteration);
        }
    }
    assert!(!buddy_core::config_lock::lock_dir(&path).exists());
}
