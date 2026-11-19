//! Shared HTTP client configuration for the data providers.

use std::time::Duration;

/// User agent the Electron main process sends (`hs-buddy/1.0`).
pub const USER_AGENT: &str = "hs-buddy/1.0";
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Select Buddy's provider independently of optional telemetry. An embedding
/// process's existing selection is retained; repeated initialization is safe.
pub fn initialize_tls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub fn client() -> reqwest::Client {
    initialize_tls();
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("reqwest client with static configuration")
}

/// `encodeURIComponent` semantics: everything is percent-encoded except
/// `A-Z a-z 0-9 - _ . ! ~ * ' ( )`, the set that function leaves untouched
/// (a superset of RFC 3986's unreserved characters).
pub fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Error text from a failed reqwest call that never echoes the URL, which may
/// carry an API key.
pub fn describe_error(err: &reqwest::Error, fallback: &str) -> String {
    if err.is_timeout() {
        return "Request timed out".to_string();
    }
    if err.is_connect() {
        return "Could not connect".to_string();
    }
    if let Some(status) = err.status() {
        return format!("HTTP {}", status.as_u16());
    }
    if err.is_decode() {
        return "Unreadable response".to_string();
    }
    fallback.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_like_encode_uri_component() {
        assert_eq!(encode_component("^GSPC"), "%5EGSPC");
        assert_eq!(encode_component("BTC-USD"), "BTC-USD");
        assert_eq!(encode_component("GC=F"), "GC%3DF");
        assert_eq!(encode_component("Raleigh, NC"), "Raleigh%2C%20NC");
    }
}
