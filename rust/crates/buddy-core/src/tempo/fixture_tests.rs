use super::*;
use serde_json::json;

struct Owned(PathBuf);
impl Owned {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("buddy-tempo-fixture-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("config.json"), "{}").unwrap();
        Self(root)
    }
    fn config(&self) -> PathBuf {
        self.0.join("config.json")
    }
    fn host(&self) -> Host {
        Host {
            actions: Some("true".into()),
            enabled: Some("1".into()),
            root: Some(self.0.clone().into_os_string()),
        }
    }
    fn marker(&self, tempo: &str, jira: &str) -> serde_json::Value {
        let value = json!({"version":1,"nonce":Uuid::new_v4().to_string(),
            "tempo_url":tempo,"jira_url":jira,"now_utc":"2026-11-02T05:30:00Z"});
        self.write(&value);
        value
    }
    fn write(&self, value: &serde_json::Value) {
        std::fs::write(self.0.join(MARKER), serde_json::to_vec(value).unwrap()).unwrap();
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        let parent = std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(
            self.0.canonicalize().unwrap().parent(),
            Some(parent.as_path())
        );
        assert!(owned_kind(&self.0, true));
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn credentials() -> Credentials {
    Credentials::new(TEMPO_TOKEN, JIRA_ORIGIN, JIRA_EMAIL, JIRA_TOKEN).unwrap()
}
fn valid(owned: &Owned) {
    owned.marker("http://127.0.0.1:12345/4/", "http://127.0.0.1:12345/");
}

#[test]
fn fixture_clients_require_the_same_region_as_the_owned_marker() {
    let owned = Owned::new();
    let mut marker = owned.marker("http://127.0.0.1:12345/4/", "http://127.0.0.1:12345/");
    marker["tempo_origin"] = json!("https://api.eu.tempo.io");
    owned.write(&marker);
    assert!(connect_with_host(&credentials(), &owned.config(), &owned.host()).is_err());
    let regional = Credentials::new_in_region(
        super::super::ApiOrigin::Europe,
        TEMPO_TOKEN,
        JIRA_ORIGIN,
        JIRA_EMAIL,
        JIRA_TOKEN,
    )
    .unwrap();
    let connection = connect_with_host(&regional, &owned.config(), &owned.host()).unwrap();
    assert_eq!(
        connection.tempo.api_origin(),
        super::super::ApiOrigin::Europe
    );
    marker["tempo_origin"] = json!("https://unrelated.invalid");
    owned.write(&marker);
    assert!(connect_with_host(&regional, &owned.config(), &owned.host()).is_err());
}

#[test]
fn explicit_owned_fixture_preserves_protected_logical_https_connection_and_clock() {
    let owned = Owned::new();
    valid(&owned);
    let credentials = credentials();
    let connection = connect_with_host(&credentials, &owned.config(), &owned.host()).unwrap();
    assert_eq!(credentials.jira_origin(), JIRA_ORIGIN);
    assert_eq!(connection.jira.origin(), "http://127.0.0.1:12345/");
    assert_eq!(connection.now.to_rfc3339(), "2026-11-02T05:30:00+00:00");
}

#[test]
fn disabled_missing_and_other_profile_ownership_cannot_create_fixture_clients() {
    let owned = Owned::new();
    let other = Owned::new();
    valid(&owned);
    valid(&other);
    for host in [
        Host {
            actions: None,
            ..owned.host()
        },
        Host {
            enabled: Some("false".into()),
            ..owned.host()
        },
        Host {
            root: None,
            ..owned.host()
        },
        Host {
            root: Some("relative".into()),
            ..owned.host()
        },
        other.host(),
    ] {
        assert!(connect_with_host(&credentials(), &owned.config(), &host).is_err());
    }
    assert!(connect_with_host(&credentials(), Path::new("relative.json"), &owned.host()).is_err());
    assert!(
        connect_with_host(&credentials(), &owned.0.join("missing.json"), &owned.host()).is_err()
    );
}

#[test]
fn every_nonfixture_credential_component_is_rejected_without_routing_it() {
    let owned = Owned::new();
    valid(&owned);
    for values in [
        (
            "different-synthetic-tempo-token",
            JIRA_ORIGIN,
            JIRA_EMAIL,
            JIRA_TOKEN,
        ),
        (
            TEMPO_TOKEN,
            "https://other-synthetic.invalid/",
            JIRA_EMAIL,
            JIRA_TOKEN,
        ),
        (
            TEMPO_TOKEN,
            JIRA_ORIGIN,
            "other-synthetic@example.invalid",
            JIRA_TOKEN,
        ),
        (
            TEMPO_TOKEN,
            JIRA_ORIGIN,
            JIRA_EMAIL,
            "different-synthetic-jira-token",
        ),
    ] {
        let credentials = Credentials::new(values.0, values.1, values.2, values.3).unwrap();
        assert!(connect_with_host(&credentials, &owned.config(), &owned.host()).is_err());
    }
}

#[test]
fn either_service_cannot_escape_numeric_loopback_or_its_exact_path() {
    let owned = Owned::new();
    for (tempo, jira) in [
        ("https://api.tempo.io/4/", "http://127.0.0.1:12345/"),
        ("http://127.0.0.1:12345/4/", "https://jira.example.invalid/"),
        ("http://localhost:12345/4/", "http://127.0.0.1:12345/"),
        ("http://127.0.0.1/4/", "http://127.0.0.1:12345/"),
        ("http://127.0.0.1:12345/4/", "http://token@127.0.0.1:12345/"),
        (
            "http://127.0.0.1:12345/4/?private=1",
            "http://127.0.0.1:12345/",
        ),
        (
            "http://127.0.0.1:12345/4/",
            "http://127.0.0.1:12345/#private",
        ),
        ("http://127.0.0.1:12345/wrong/", "http://127.0.0.1:12345/"),
        ("http://127.0.0.1:12345/4/", "http://127.0.0.1:12345/rest/"),
    ] {
        owned.marker(tempo, jira);
        assert!(connect_with_host(&credentials(), &owned.config(), &owned.host()).is_err());
    }
}

#[test]
fn unsupported_marker_and_clock_bytes_are_preserved_instead_of_falling_back() {
    let owned = Owned::new();
    let base = owned.marker("http://127.0.0.1:12345/4/", "http://127.0.0.1:12345/");
    for (key, value) in [
        ("version", json!(2)),
        ("nonce", json!(Uuid::nil().to_string())),
        ("unknown", json!(true)),
        ("now_utc", json!("")),
        ("now_utc", json!("0000-01-01T00:00:00Z")),
        ("now_utc", json!("2016-12-31T23:59:60Z")),
        ("now_utc", json!("+020000-01-01T00:00:00Z")),
    ] {
        let mut value_copy = base.clone();
        value_copy[key] = value;
        owned.write(&value_copy);
        let bytes = std::fs::read(owned.0.join(MARKER)).unwrap();
        assert!(connect_with_host(&credentials(), &owned.config(), &owned.host()).is_err());
        assert_eq!(std::fs::read(owned.0.join(MARKER)).unwrap(), bytes);
    }
    std::fs::write(owned.0.join(MARKER), vec![b'x'; MAX_MARKER as usize + 1]).unwrap();
    assert!(connect_with_host(&credentials(), &owned.config(), &owned.host()).is_err());
    assert_eq!(
        std::fs::metadata(owned.0.join(MARKER)).unwrap().len(),
        MAX_MARKER + 1
    );
}

#[test]
fn required_clock_normalizes_offset_and_missing_clock_is_rejected() {
    let owned = Owned::new();
    let mut marker = owned.marker("http://127.0.0.1:12345/4/", "http://127.0.0.1:12345/");
    marker.as_object_mut().unwrap().remove("now_utc");
    owned.write(&marker);
    assert!(connect_with_host(&credentials(), &owned.config(), &owned.host()).is_err());
    marker["now_utc"] = json!("2026-11-02T01:30:00-04:00");
    owned.write(&marker);
    let connection = connect_with_host(&credentials(), &owned.config(), &owned.host()).unwrap();
    assert_eq!(connection.now.to_rfc3339(), "2026-11-02T05:30:00+00:00");
}

#[tokio::test]
async fn validated_dual_clients_perform_real_scoped_jira_and_tempo_reads_only() {
    use crate::tempo::jira::tests::{Fixture, reply};
    let profile = json!({"accountId":"synthetic","timeZone":"America/Winnipeg"}).to_string();
    let page = json!({"self":"https://api.tempo.io/4/worklogs","metadata":{"count":0,"limit":50,"offset":0,"next":null},"results":[]}).to_string();
    let server = Fixture::new(vec![reply(200, &profile), reply(200, &page)]);
    let owned = Owned::new();
    owned.marker(
        server.base.join("4/").unwrap().as_str(),
        server.base.as_str(),
    );
    let connection = connect_with_host(&credentials(), &owned.config(), &owned.host()).unwrap();
    let profile = connection.jira.myself().await.unwrap();
    assert_eq!(profile.account.as_str(), "synthetic");
    let range =
        super::super::DateRange::new("2026-11-02".parse().unwrap(), "2026-11-03".parse().unwrap())
            .unwrap();
    assert!(
        connection
            .tempo
            .worklogs(profile.account, range)
            .await
            .unwrap()
            .is_empty()
    );
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /rest/api/3/myself "));
    assert!(requests[1].starts_with("GET /4/worklogs/user/synthetic?"));
    assert!(requests[1].contains("from=2026-11-02") && requests[1].contains("to=2026-11-03"));
}
