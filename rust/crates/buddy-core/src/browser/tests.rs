use std::collections::BTreeMap;
use std::sync::Mutex;

use super::*;

fn url(raw: &str) -> Url {
    Url::parse(raw).unwrap()
}

fn fixture() -> FixtureOrigin {
    FixtureOrigin::parse(Some("http://127.0.0.1:8822/"), true).unwrap()
}

#[test]
fn only_public_http_and_https_pages_are_web_targets() {
    for raw in [
        "https://example.com/",
        "http://example.com/path?q=1#frag",
        "https://sub.example.co.uk:8443/",
        "https://93.184.215.14/",
        "https://[2606:2800:21f:cb07:6820:80da:af6b:8b2c]/",
    ] {
        assert!(matches!(classify(raw, None), Ok(Target::Web(_))), "{raw}");
    }
}

#[test]
fn other_schemes_are_refused_or_offered_externally() {
    for (raw, scheme) in [
        ("file:///C:/Windows/win.ini", "file"),
        ("javascript:alert(1)", "javascript"),
        ("data:text/html,<b>x</b>", "data"),
        ("ms-settings:privacy", "ms-settings"),
        ("ftp://example.com/", "ftp"),
        ("chrome://settings", "chrome"),
        ("about:config", "about"),
    ] {
        assert_eq!(
            classify(raw, None),
            Err(Blocked::Scheme(scheme.into())),
            "{raw}"
        );
    }
    assert!(matches!(
        classify("mailto:someone@example.com", None),
        Ok(Target::External(_))
    ));
    assert_eq!(classify("about:blank", None), Ok(Target::Blank));
    assert_eq!(classify("about:srcdoc", None), Ok(Target::Blank));
    assert_eq!(classify("not a url", None), Err(Blocked::Invalid));
    assert_eq!(
        classify("https://user:pw@example.com/", None),
        Err(Blocked::Invalid)
    );
}

#[test]
fn internal_hosts_and_private_addresses_are_refused() {
    for raw in [
        "http://localhost/",
        "http://LOCALHOST./",
        "http://app.localhost/",
        "http://printer.local/",
        "http://db.internal/",
        "http://router.home.arpa/",
        "http://intranet/",
        "http://127.0.0.1/",
        "http://127.8.9.10/",
        "http://10.0.0.1/",
        "http://172.16.0.1/",
        "http://172.31.255.255/",
        "http://192.168.1.1/",
        "http://169.254.169.254/latest/meta-data",
        "http://100.64.0.1/",
        "http://100.127.255.255/",
        "http://0.0.0.0/",
        "http://224.0.0.1/",
        "http://[::1]/",
        "http://[::]/",
        "http://[fc00::1]/",
        "http://[fd12:3456::1]/",
        "http://[fe80::1]/",
        "http://[::ffff:127.0.0.1]/",
        "http://[::ffff:10.1.2.3]/",
        // Decimal and hex encodings normalize to the private address.
        "http://2130706433/",
        "http://0x7f000001/",
        "http://017700000001/",
    ] {
        assert_eq!(classify(raw, None), Err(Blocked::Internal), "{raw}");
    }
    for raw in [
        "http://172.15.0.1/",
        "http://172.32.0.1/",
        "http://100.63.0.1/",
        "http://100.128.0.1/",
    ] {
        assert!(matches!(classify(raw, None), Ok(Target::Web(_))), "{raw}");
    }
}

#[test]
fn the_fixture_origin_needs_an_isolated_config_and_matches_exactly() {
    assert!(FixtureOrigin::parse(Some("http://127.0.0.1:8822/"), false).is_none());
    for value in [
        "http://localhost:8822/",
        "http://127.0.0.2:8822/",
        "https://127.0.0.1:8822/",
        "http://127.0.0.1/",
        "http://127.0.0.1:8822/path",
        "http://u@127.0.0.1:8822/",
        "http://10.0.0.1:8822/",
    ] {
        assert!(FixtureOrigin::parse(Some(value), true).is_none(), "{value}");
    }
    let origin = fixture();
    assert!(matches!(
        classify("http://127.0.0.1:8822/ok?x=1", Some(&origin)),
        Ok(Target::Web(_))
    ));
    assert_eq!(
        classify("http://127.0.0.1:8823/", Some(&origin)),
        Err(Blocked::Internal)
    );
    assert_eq!(
        classify("http://127.0.0.2:8822/", Some(&origin)),
        Err(Blocked::Internal)
    );
    assert_eq!(
        classify("http://127.0.0.1:8822/", None),
        Err(Blocked::Internal)
    );
}

/// Answers from a table and counts lookups; anything else fails to resolve.
struct FakeLookup(
    BTreeMap<&'static str, std::io::Result<Vec<IpAddr>>>,
    Mutex<Vec<String>>,
);

impl FakeLookup {
    fn new(entries: Vec<(&'static str, std::io::Result<Vec<IpAddr>>)>) -> Arc<Self> {
        Arc::new(Self(entries.into_iter().collect(), Mutex::default()))
    }

    fn calls(&self) -> Vec<String> {
        self.1.lock().unwrap().clone()
    }
}

impl Lookup for FakeLookup {
    fn lookup(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>> {
        self.1.lock().unwrap().push(format!("{host}:{port}"));
        match self.0.get(host) {
            Some(Ok(addresses)) => Ok(addresses.clone()),
            Some(Err(err)) => Err(std::io::Error::new(err.kind(), "fake")),
            None => Err(std::io::ErrorKind::NotFound.into()),
        }
    }
}

fn ip(raw: &str) -> IpAddr {
    raw.parse().unwrap()
}

fn table() -> Arc<FakeLookup> {
    FakeLookup::new(vec![
        ("public.example", Ok(vec![ip("93.184.215.14")])),
        (
            "rebind.example",
            Ok(vec![ip("93.184.215.14"), ip("192.168.1.10")]),
        ),
        ("mapped.example", Ok(vec![ip("::ffff:127.0.0.1")])),
        ("tailnet.example", Ok(vec![ip("100.101.102.103")])),
        ("empty.example", Ok(vec![])),
        ("down.example", Err(std::io::ErrorKind::TimedOut.into())),
    ])
}

#[test]
fn names_that_resolve_inside_are_refused_before_anything_loads() {
    let lookup = table();
    let checker = Checker::new(lookup.clone(), None);
    assert_eq!(checker.check(&url("https://public.example/")), Ok(()));
    for name in ["rebind", "mapped", "tailnet"] {
        let target = format!("https://{name}.example/");
        assert_eq!(
            checker.check(&url(&target)),
            Err(Blocked::Internal),
            "{name}"
        );
        assert_eq!(
            checker.navigation(&target),
            Step::Block(Blocked::Internal),
            "{name}"
        );
    }
    for name in ["empty", "down", "missing"] {
        let target = format!("http://{name}.example/");
        assert_eq!(
            checker.check(&url(&target)),
            Err(Blocked::Unresolved),
            "{name}"
        );
    }
    let calls = lookup.calls();
    assert!(calls.contains(&"public.example:443".to_owned()));
    assert!(calls.contains(&"down.example:80".to_owned()));
}

#[test]
fn verdicts_are_reused_until_they_expire() {
    let lookup = table();
    let checker = Checker::new(lookup.clone(), None);
    for _ in 0..3 {
        assert_eq!(checker.navigation("https://PUBLIC.example/a"), Step::Allow);
        assert_eq!(
            checker.navigation("https://rebind.example/"),
            Step::Block(Blocked::Internal)
        );
    }
    assert_eq!(lookup.calls().len(), 2);

    let lookup = table();
    let expired = Checker::with_ttl(lookup.clone(), None, Duration::ZERO);
    for _ in 0..3 {
        assert_eq!(expired.check(&url("https://public.example/")), Ok(()));
    }
    assert_eq!(lookup.calls().len(), 3);
}

#[test]
fn literals_and_the_fixture_need_no_lookup() {
    let lookup = table();
    let checker = Checker::new(lookup.clone(), Some(fixture()));
    assert_eq!(checker.check(&url("https://93.184.215.14/")), Ok(()));
    assert_eq!(checker.navigation("http://127.0.0.1:8822/ok"), Step::Allow);
    assert_eq!(
        checker.navigation("http://127.0.0.2:8822/"),
        Step::Block(Blocked::Internal)
    );
    assert!(lookup.calls().is_empty());
}

#[test]
fn every_navigation_gets_the_scheme_and_host_check_first() {
    let lookup = table();
    let checker = Checker::new(lookup.clone(), None);
    assert_eq!(checker.navigation("about:srcdoc"), Step::Allow);
    assert_eq!(
        checker.navigation("file:///etc/passwd"),
        Step::Block(Blocked::Scheme("file".into()))
    );
    assert_eq!(
        checker.navigation("http://10.0.0.1/"),
        Step::Block(Blocked::Internal)
    );
    assert!(matches!(
        checker.navigation("mailto:a@example.com"),
        Step::External(_)
    ));
    assert!(lookup.calls().is_empty());
}

#[test]
fn the_system_lookup_gives_up_after_its_timeout() {
    // A name under the reserved `invalid` domain never resolves; it must fail,
    // not hang, whatever the resolver does with it.
    let lookup = SystemLookup {
        timeout: Duration::from_millis(1),
    };
    assert!(lookup.lookup("buddy-browser-test.invalid", 443).is_err());
}

#[test]
fn audit_keeps_only_the_origin() {
    let audit = Audit::new("https://example.com/secret/path?token=abc", &Blocked::Popup);
    assert_eq!(audit.kind, "popup");
    assert_eq!(audit.origin, "https://example.com");
    assert_eq!(
        Audit::new("file:///C:/x", &Blocked::Scheme("file".into())).origin,
        "file:"
    );
    assert_eq!(
        Audit::new(
            "https://a.example/",
            &Blocked::Permission("geolocation".into())
        )
        .reason,
        "geolocation"
    );
    assert_eq!(Audit::new("::", &Blocked::Invalid).origin, "invalid");
}

#[test]
fn every_refusal_has_user_text() {
    for reason in [
        Blocked::Invalid,
        Blocked::Scheme("file".into()),
        Blocked::Internal,
        Blocked::Unresolved,
        Blocked::Popup,
        Blocked::Download,
        Blocked::Permission("camera".into()),
    ] {
        assert!(reason.to_string().ends_with('.'), "{reason:?}");
    }
}

#[test]
fn session_round_trips_and_drops_disallowed_tabs() {
    let mut config = AppConfig::default();
    let session = Session {
        tabs: vec![
            Tab {
                url: "https://example.com/".into(),
                title: "Example".into(),
            },
            Tab {
                url: "https://docs.rs/".into(),
                title: String::new(),
            },
        ],
        active: Some(1),
        zoom: 1.25,
    };
    session.store(&mut config);
    assert_eq!(Session::load(&config, None), session);

    config.native.extra.insert(
        "browser".into(),
        serde_json::json!({
            "tabs": [
                {"url": "file:///etc/passwd"},
                {"url": "http://127.0.0.1:8822/"},
                {"url": "https://example.com/", "title": "Kept"},
            ],
            "active": 7,
            "zoom": 99.0,
        }),
    );
    let loaded = Session::load(&config, None);
    assert_eq!(
        loaded.tabs,
        vec![Tab {
            url: "https://example.com/".into(),
            title: "Kept".into()
        }]
    );
    assert_eq!(loaded.active, Some(0));
    assert_eq!(loaded.zoom, ZOOM_RANGE.1);
}

#[test]
fn fixture_tabs_survive_only_with_the_fixture() {
    let mut config = AppConfig::default();
    Session {
        tabs: vec![Tab {
            url: "http://127.0.0.1:8822/ok".into(),
            title: "Fixture".into(),
        }],
        active: Some(0),
        zoom: 1.0,
    }
    .store(&mut config);
    assert_eq!(Session::load(&config, Some(&fixture())).tabs.len(), 1);
    assert!(Session::load(&config, None).tabs.is_empty());
}

#[test]
fn a_damaged_session_loads_empty_and_other_settings_survive() {
    let mut config = AppConfig::default();
    config
        .native
        .extra
        .insert("browser".into(), serde_json::json!("garbage"));
    config
        .native
        .extra
        .insert("other".into(), serde_json::json!(42));
    let loaded = Session::load(&config, None);
    assert!(loaded.tabs.is_empty());
    assert_eq!(loaded.zoom, 1.0);
    loaded.store(&mut config);
    assert_eq!(config.native.extra["other"], 42);
}

#[test]
fn many_tabs_are_capped() {
    let mut config = AppConfig::default();
    let tabs = (0..30)
        .map(|n| Tab {
            url: format!("https://example.com/{n}"),
            title: String::new(),
        })
        .collect();
    Session {
        tabs,
        active: Some(29),
        zoom: 1.0,
    }
    .store(&mut config);
    let loaded = Session::load(&config, None);
    assert_eq!(loaded.tabs.len(), MAX_TABS);
    assert_eq!(loaded.active, Some(MAX_TABS - 1));
}

#[test]
fn zoom_is_clamped() {
    assert_eq!(clamp_zoom(0.1), ZOOM_RANGE.0);
    assert_eq!(clamp_zoom(f64::NAN), 1.0);
    assert_eq!(clamp_zoom(1.5), 1.5);
}

#[test]
fn the_profile_lives_beside_the_config() {
    let profile = profile_dir(Path::new("/tmp/isolated/config.json"));
    assert_eq!(profile, Path::new("/tmp/isolated/browser-profile"));
    assert_eq!(data_store_id(&profile), data_store_id(&profile));
    assert_ne!(
        data_store_id(&profile),
        data_store_id(Path::new("/other/browser-profile"))
    );
}

#[test]
fn the_bookmark_list_selection_survives_a_restart() {
    let mut config = AppConfig::default();
    Session {
        tabs: vec![Tab {
            url: "https://example.com/".into(),
            title: String::new(),
        }],
        active: None,
        zoom: 1.0,
    }
    .store(&mut config);
    assert_eq!(Session::load(&config, None).active, None);
    // A session saved before the list had its own selection picks its tab.
    config.native.extra.insert(
        "browser".into(),
        serde_json::json!({"tabs": [{"url": "https://example.com/"}], "active": 0}),
    );
    assert_eq!(Session::load(&config, None).active, Some(0));
}
