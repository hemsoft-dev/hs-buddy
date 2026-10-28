//! What an embedded browser tab may do. Pure policy, independent of the
//! platform webview: buddy-app asks it about every navigation, popup,
//! download and permission request, and records each refusal.
//!
//! The rules match the Electron in-app browser window
//! (`electron/ipc/shellHandlers.ts`) and are stricter than its embedded tab:
//! only http/https, no internal hosts, every top-level navigation is
//! re-checked against DNS before it loads, and popups, downloads and
//! permission requests are always refused.

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Host;
pub use url::Url;

use crate::config::AppConfig;

/// Why the browser refused something. `Display` is the text a tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    Invalid,
    Scheme(String),
    Internal,
    Unresolved,
    Popup,
    Download,
    Permission(String),
}

impl fmt::Display for Blocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Blocked::Invalid => f.write_str("That address isn't a valid web address."),
            Blocked::Scheme(scheme) => write!(
                f,
                "Buddy only opens http and https pages, not {scheme}: links."
            ),
            Blocked::Internal => {
                f.write_str("Buddy doesn't open local or private network addresses.")
            }
            Blocked::Unresolved => f.write_str("The site's address couldn't be looked up."),
            Blocked::Popup => f.write_str("A pop-up window was blocked."),
            Blocked::Download => {
                f.write_str("Downloads are blocked. Open the page externally to download.")
            }
            Blocked::Permission(kind) => {
                write!(f, "The page asked for {kind} access, which was denied.")
            }
        }
    }
}

/// A syntactically allowed destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// An http/https page that still needs its DNS check.
    Web(Url),
    /// The page's own blank or srcdoc document.
    Blank,
    /// A `mailto:` link, offered to the system handler instead.
    External(Url),
}

/// The loopback origin a qualification fixture serves from. Only honored
/// with an isolated `BUDDY_CONFIG_PATH`, and only for this exact origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureOrigin(Url);

pub const FIXTURE_ORIGIN_ENV: &str = "BUDDY_BROWSER_TEST_ORIGIN";

impl FixtureOrigin {
    pub fn from_env() -> Option<Self> {
        Self::parse(
            std::env::var(FIXTURE_ORIGIN_ENV).ok().as_deref(),
            std::env::var_os(crate::config::CONFIG_PATH_ENV).is_some(),
        )
    }

    fn parse(value: Option<&str>, isolated_config: bool) -> Option<Self> {
        if !isolated_config {
            return None;
        }
        let url = Url::parse(value?).ok()?;
        let loopback = url.host() == Some(Host::Ipv4(Ipv4Addr::LOCALHOST));
        let bare = url.path() == "/" && url.query().is_none() && url.fragment().is_none();
        (url.scheme() == "http"
            && loopback
            && url.port().is_some()
            && bare
            && url.username().is_empty())
        .then_some(Self(url))
    }

    fn admits(&self, url: &Url) -> bool {
        url.scheme() == self.0.scheme()
            && url.host() == self.0.host()
            && url.port_or_known_default() == self.0.port_or_known_default()
    }
}

/// The scheme and host check every request passes first.
pub fn classify(raw: &str, fixture: Option<&FixtureOrigin>) -> Result<Target, Blocked> {
    let url = Url::parse(raw.trim()).map_err(|_| Blocked::Invalid)?;
    match url.scheme() {
        "about" if matches!(url.path(), "blank" | "srcdoc") => Ok(Target::Blank),
        "mailto" => Ok(Target::External(url)),
        "http" | "https" => {
            if fixture.is_some_and(|origin| origin.admits(&url)) {
                return Ok(Target::Web(url));
            }
            if !url.username().is_empty() || url.password().is_some() {
                return Err(Blocked::Invalid);
            }
            match url.host() {
                None => Err(Blocked::Invalid),
                Some(host) if internal_host(&host) => Err(Blocked::Internal),
                Some(_) => Ok(Target::Web(url)),
            }
        }
        scheme => Err(Blocked::Scheme(scheme.to_owned())),
    }
}

fn internal_host(host: &Host<&str>) -> bool {
    match host {
        Host::Ipv4(ip) => private_ip(IpAddr::V4(*ip)),
        Host::Ipv6(ip) => private_ip(IpAddr::V6(*ip)),
        Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost"
                || [".localhost", ".local", ".internal", ".home.arpa"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
                // A single label never names a public site and may reach an intranet host.
                || !name.contains('.')
        }
    }
}

/// Loopback, private, link-local, carrier-grade NAT (Tailscale), multicast
/// and unspecified addresses, including IPv4 written as IPv6.
pub fn private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => private_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) => private_v4(v4),
            None => private_v6(ip),
        },
    }
}

fn private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b))
}

fn private_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || first & 0xfe00 == 0xfc00
        || first & 0xffc0 == 0xfe80
}

/// Name lookups the engine's navigation callback can afford: blocking, but
/// bounded. Replaceable in tests.
pub trait Lookup: Send + Sync {
    fn lookup(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>>;
}

/// The operating system's resolver, given up on after `timeout`.
pub struct SystemLookup {
    pub timeout: Duration,
}

impl Lookup for SystemLookup {
    fn lookup(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let host = host.to_owned();
        std::thread::spawn(move || {
            let result = (host.as_str(), port)
                .to_socket_addrs()
                .map(|addresses| addresses.map(|address| address.ip()).collect());
            let _ = sender.send(result);
        });
        receiver
            .recv_timeout(self.timeout)
            .unwrap_or_else(|_| Err(std::io::ErrorKind::TimedOut.into()))
    }
}

/// How long a lookup may hold a navigation, and how long its verdict is reused.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);
pub const VERDICT_TTL: Duration = Duration::from_secs(60);

type Verdicts = HashMap<(String, u16), (Instant, Result<(), Blocked>)>;

/// Decides every navigation before the engine sends any request. The engine
/// asks synchronously and, on WebKit, for subframes too without saying which
/// frame is navigating, so the decision can't wait for an asynchronous lookup
/// and reload afterwards: it resolves the name here, bounded by the lookup's
/// timeout, and reuses each verdict for a while. A name that resolves to any
/// internal address is refused, so a public name pointing at the local network
/// can't be used to reach it.
pub struct Checker {
    lookup: Arc<dyn Lookup>,
    fixture: Option<FixtureOrigin>,
    ttl: Duration,
    verdicts: Mutex<Verdicts>,
}

impl Checker {
    pub fn new(lookup: Arc<dyn Lookup>, fixture: Option<FixtureOrigin>) -> Self {
        Self::with_ttl(lookup, fixture, VERDICT_TTL)
    }

    pub fn with_ttl(
        lookup: Arc<dyn Lookup>,
        fixture: Option<FixtureOrigin>,
        ttl: Duration,
    ) -> Self {
        Self {
            lookup,
            fixture,
            ttl,
            verdicts: Mutex::default(),
        }
    }

    pub fn fixture(&self) -> Option<&FixtureOrigin> {
        self.fixture.as_ref()
    }

    /// The scheme and host check, then the name check.
    pub fn navigation(&self, raw: &str) -> Step {
        match classify(raw, self.fixture.as_ref()) {
            Err(reason) => Step::Block(reason),
            Ok(Target::Blank) => Step::Allow,
            Ok(Target::External(url)) => Step::External(url),
            Ok(Target::Web(url)) => match self.check(&url) {
                Ok(()) => Step::Allow,
                Err(reason) => Step::Block(reason),
            },
        }
    }

    /// Resolves the page's name, or reuses a verdict younger than the TTL.
    pub fn check(&self, url: &Url) -> Result<(), Blocked> {
        if self
            .fixture
            .as_ref()
            .is_some_and(|origin| origin.admits(url))
        {
            return Ok(());
        }
        let Some(Host::Domain(name)) = url.host() else {
            // IP literals were already checked by `classify`.
            return Ok(());
        };
        let key = (
            name.to_ascii_lowercase(),
            url.port_or_known_default().unwrap_or(443),
        );
        let now = Instant::now();
        if let Some((at, verdict)) = self.verdicts().get(&key)
            && now.duration_since(*at) < self.ttl
        {
            return verdict.clone();
        }
        let verdict = match self.lookup.lookup(&key.0, key.1) {
            Err(_) => Err(Blocked::Unresolved),
            Ok(addresses) if addresses.is_empty() => Err(Blocked::Unresolved),
            Ok(addresses) if addresses.iter().copied().any(private_ip) => Err(Blocked::Internal),
            Ok(_) => Ok(()),
        };
        self.verdicts().insert(key, (now, verdict.clone()));
        verdict
    }

    fn verdicts(&self) -> std::sync::MutexGuard<'_, Verdicts> {
        // A panic elsewhere can't leave a verdict half-written; keep using it.
        self.verdicts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// What a tab does with one navigation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Let the engine continue.
    Allow,
    /// Cancel and tell the user why.
    Block(Blocked),
    /// Cancel and offer the link to the system handler.
    External(Url),
}

/// One refused request, as written to the qualification audit file. Only
/// the origin is kept, never the path or query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Audit {
    pub kind: &'static str,
    pub origin: String,
    pub reason: String,
}

impl Audit {
    pub fn new(raw: &str, reason: &Blocked) -> Self {
        let kind = match reason {
            Blocked::Invalid => "invalid",
            Blocked::Scheme(_) => "scheme",
            Blocked::Internal => "internal",
            Blocked::Unresolved => "unresolved",
            Blocked::Popup => "popup",
            Blocked::Download => "download",
            Blocked::Permission(_) => "permission",
        };
        let origin = match Url::parse(raw) {
            Ok(url) if url.has_host() => url.origin().ascii_serialization(),
            Ok(url) => format!("{}:", url.scheme()),
            Err(_) => "invalid".into(),
        };
        let reason = match reason {
            Blocked::Scheme(scheme) => scheme.clone(),
            Blocked::Permission(permission) => permission.clone(),
            _ => String::new(),
        };
        Self {
            kind,
            origin,
            reason,
        }
    }
}

/// Open tabs, restored on the next start.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub tabs: Vec<Tab>,
    /// The selected tab; `None` is the bookmark list.
    #[serde(default)]
    pub active: Option<usize>,
    #[serde(default = "default_zoom")]
    pub zoom: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    pub url: String,
    #[serde(default)]
    pub title: String,
}

pub const MAX_TABS: usize = 20;
pub const ZOOM_RANGE: (f64, f64) = (0.5, 3.0);
const SESSION_KEY: &str = "browser";

fn default_zoom() -> f64 {
    1.0
}

impl Session {
    /// The saved session, without tabs that are no longer allowed. A
    /// damaged entry yields an empty session rather than an error.
    pub fn load(config: &AppConfig, fixture: Option<&FixtureOrigin>) -> Self {
        let mut session: Session = config
            .native
            .extra
            .get(SESSION_KEY)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_else(|| Session {
                zoom: default_zoom(),
                ..Session::default()
            });
        session
            .tabs
            .retain(|tab| matches!(classify(&tab.url, fixture), Ok(Target::Web(_))));
        session.tabs.truncate(MAX_TABS);
        session.active = session
            .active
            .map(|active| active.min(session.tabs.len().saturating_sub(1)))
            .filter(|_| !session.tabs.is_empty());
        session.zoom = clamp_zoom(session.zoom);
        session
    }

    pub fn store(&self, config: &mut AppConfig) {
        let value = serde_json::to_value(self).unwrap_or(Value::Null);
        config.native.extra.insert(SESSION_KEY.into(), value);
    }
}

pub fn clamp_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() {
        zoom.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1)
    } else {
        default_zoom()
    }
}

/// The app-private engine profile (cookies, cache), kept beside the config
/// so an isolated `BUDDY_CONFIG_PATH` run never touches the real one.
pub fn profile_dir(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("browser-profile")
}

/// macOS keys a persistent WebKit data store by a 16-byte identifier
/// instead of a folder; derive it from the profile folder.
pub fn data_store_id(profile: &Path) -> [u8; 16] {
    let digest = Sha256::digest(profile.to_string_lossy().as_bytes());
    let mut id = [0; 16];
    id.copy_from_slice(&digest[..16]);
    id
}

#[cfg(test)]
mod tests;
