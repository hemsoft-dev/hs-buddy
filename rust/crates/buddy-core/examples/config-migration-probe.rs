//! Synthetic stdin/stdout bridge for the Electron rollback contract. Uses only
//! a disposable profile and refuses plaintext weather to avoid any OS vault.
use std::io::Read;

fn main() {
    let mut body = String::new();
    std::io::stdin().read_to_string(&mut body).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(&path, body).unwrap();
    let (mut config, stamp) = buddy_core::config::AppConfig::load_with_stamp_from(&path).unwrap();
    assert!(
        config.ui.weather_location.is_none(),
        "synthetic ciphertext fixture required"
    );
    config.set_dashboard_card_visible("weather", false);
    config.native.active_section = Some("settings".into());
    assert!(config.save_if_unchanged_at(&path, &stamp).unwrap());
    print!("{}", std::fs::read_to_string(&path).unwrap());
}
