use super::*;
use serde_json::json;

struct Owned(PathBuf);
impl Owned {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("buddy-todoist-fixture-{}", Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("config.json"), "{}").unwrap();
        Self(path)
    }
    fn host(&self) -> Host {
        Host {
            github_actions: Some("true".into()),
            enabled: Some("1".into()),
            runner_temp: Some(self.0.clone().into_os_string()),
        }
    }
    fn config(&self) -> PathBuf {
        self.0.join("config.json")
    }
    fn marker(&self, url: &str) {
        std::fs::write(
            self.0.join(MARKER),
            serde_json::to_vec(&json!({"version":1,"nonce":Uuid::new_v4(),"api_url":url,"now_utc":"2026-11-02T05:30:00Z"})).unwrap(),
        )
        .unwrap();
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        let root = std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(
            self.0.canonicalize().unwrap().parent(),
            Some(root.as_path())
        );
        assert!(
            !std::fs::symlink_metadata(&self.0)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn explicit_owned_loopback_fixture_is_accepted_without_process_environment_changes() {
    let owned = Owned::new();
    owned.marker("http://127.0.0.1:12345/api/v1/");
    let connection = validate(TOKEN, &owned.config(), &owned.host()).unwrap();
    assert_eq!(connection.api.as_str(), "http://127.0.0.1:12345/api/v1/");
    assert_eq!(connection.now.to_rfc3339(), "2026-11-02T05:30:00+00:00");
}

#[test]
fn real_tokens_disabled_hosts_and_missing_ownership_fail_before_any_request() {
    let owned = Owned::new();
    owned.marker("http://127.0.0.1:12345/api/v1/");
    assert_eq!(
        validate("not-a-fixture-token", &owned.config(), &owned.host()),
        Err(TransportError::FixtureUnavailable)
    );
    let mut host = owned.host();
    host.github_actions = Some("false".into());
    assert_eq!(
        validate(TOKEN, &owned.config(), &host),
        Err(TransportError::FixtureUnavailable)
    );
    host = owned.host();
    host.enabled = None;
    assert_eq!(
        validate(TOKEN, &owned.config(), &host),
        Err(TransportError::FixtureUnavailable)
    );
    host = owned.host();
    host.runner_temp = None;
    assert_eq!(
        validate(TOKEN, &owned.config(), &host),
        Err(TransportError::FixtureUnavailable)
    );
    assert_eq!(
        validate(TOKEN, Path::new("relative.json"), &owned.host()),
        Err(TransportError::FixtureUnavailable)
    );
}

#[test]
fn other_profile_and_upstream_or_credential_bearing_origins_cannot_become_fixture_routes() {
    let owned = Owned::new();
    let other = Owned::new();
    other.marker("http://127.0.0.1:12345/api/v1/");
    assert_eq!(
        validate(TOKEN, &other.config(), &owned.host()),
        Err(TransportError::FixtureUnavailable)
    );
    for url in [
        "https://api.todoist.com/api/v1/",
        "http://localhost:12345/api/v1/",
        "http://127.0.0.1/api/v1/",
        "http://127.0.0.1:12345/api/v1/?private=yes",
        "http://token@127.0.0.1:12345/api/v1/",
        "http://127.0.0.1:12345/api/v1/#private",
        "http://127.0.0.1:12345/wrong/",
    ] {
        owned.marker(url);
        assert_eq!(
            validate(TOKEN, &owned.config(), &owned.host()),
            Err(TransportError::FixtureUnavailable)
        );
    }
}

#[test]
fn malformed_oversized_and_unsupported_markers_are_preserved_and_rejected() {
    let owned = Owned::new();
    for value in [
        json!(null),
        json!({"version":2,"nonce":Uuid::new_v4(),"api_url":"http://127.0.0.1:12345/api/v1/","now_utc":"2026-11-02T05:30:00Z"}),
        json!({"version":1,"nonce":Uuid::nil(),"api_url":"http://127.0.0.1:12345/api/v1/","now_utc":"2026-11-02T05:30:00Z"}),
        json!({"version":1,"nonce":Uuid::new_v4(),"api_url":"http://127.0.0.1:12345/api/v1/","now_utc":"2026-11-02T05:30:00Z","unknown":true}),
    ] {
        let bytes = serde_json::to_vec(&value).unwrap();
        std::fs::write(owned.0.join(MARKER), &bytes).unwrap();
        assert_eq!(
            validate(TOKEN, &owned.config(), &owned.host()),
            Err(TransportError::FixtureUnavailable)
        );
        assert_eq!(std::fs::read(owned.0.join(MARKER)).unwrap(), bytes);
    }
    std::fs::write(owned.0.join(MARKER), vec![b'x'; MAX_MARKER as usize + 1]).unwrap();
    assert_eq!(
        validate(TOKEN, &owned.config(), &owned.host()),
        Err(TransportError::FixtureUnavailable)
    );
    assert_eq!(
        std::fs::metadata(owned.0.join(MARKER)).unwrap().len(),
        MAX_MARKER + 1
    );
}

#[test]
fn fixture_clock_is_required_bounded_and_normalized_without_host_clock_fallback() {
    let owned = Owned::new();
    for clock in [
        json!(null),
        json!(""),
        json!("2026-11-02"),
        json!("+020000-01-01T00:00:00Z"),
        json!("2016-12-31T23:59:60Z"),
        json!("0000-01-01T00:00:00Z"),
    ] {
        owned.marker("http://127.0.0.1:12345/api/v1/");
        let mut marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(owned.0.join(MARKER)).unwrap()).unwrap();
        marker["now_utc"] = clock;
        std::fs::write(owned.0.join(MARKER), serde_json::to_vec(&marker).unwrap()).unwrap();
        assert_eq!(
            validate(TOKEN, &owned.config(), &owned.host()),
            Err(TransportError::FixtureUnavailable)
        );
    }
    owned.marker("http://127.0.0.1:12345/api/v1/");
    let mut marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(owned.0.join(MARKER)).unwrap()).unwrap();
    marker.as_object_mut().unwrap().remove("now_utc");
    std::fs::write(owned.0.join(MARKER), serde_json::to_vec(&marker).unwrap()).unwrap();
    assert_eq!(
        validate(TOKEN, &owned.config(), &owned.host()),
        Err(TransportError::FixtureUnavailable)
    );
    marker["now_utc"] = json!("2026-11-02T01:30:00-04:00");
    std::fs::write(owned.0.join(MARKER), serde_json::to_vec(&marker).unwrap()).unwrap();
    assert_eq!(
        validate(TOKEN, &owned.config(), &owned.host())
            .unwrap()
            .now
            .to_rfc3339(),
        "2026-11-02T05:30:00+00:00"
    );
}
