//! Which URLs a bookmark may hold, open or fetch: http and https only, never an
//! internal host. Mirrors `validateUrl` in `src/utils/networkSecurity.ts`, so the
//! embedded browser can apply the same rules.
use std::net::IpAddr;

use reqwest::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    #[error("Please enter a valid URL (e.g., https://example.com)")]
    Invalid,
    #[error("Only http and https URLs are allowed")]
    Protocol,
    #[error("Internal or private network URLs are not allowed")]
    Internal,
}

const BLOCKED_HOSTNAMES: [&str; 4] = ["localhost", "127.0.0.1", "[::1]", "::1"];
const INTERNAL_PREFIXES: [&str; 4] = ["127.", "169.254.", "10.", "192.168."];
const INTERNAL_SUFFIXES: [&str; 2] = [".local", ".internal"];

/// The parsed URL when it is http or https and its host is not internal.
pub fn validate_url(text: &str) -> Result<Url, UrlError> {
    let parsed = Url::parse(text).map_err(|_| UrlError::Invalid)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(UrlError::Protocol);
    }
    let host = parsed
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or(UrlError::Invalid)?
        .to_ascii_lowercase();
    // Electron checks the host text only. An IP literal is also checked by range,
    // which catches `0.0.0.0`, private IPv6 and IPv4-mapped addresses.
    if is_internal_hostname(&host) || ip_literal(&host).is_some_and(is_private_ip) {
        return Err(UrlError::Internal);
    }
    Ok(parsed)
}

/// `isInternalHostname`: loopback names, private IPv4 prefixes, `.local` and `.internal`.
pub fn is_internal_hostname(hostname: &str) -> bool {
    BLOCKED_HOSTNAMES.contains(&hostname)
        || INTERNAL_PREFIXES
            .iter()
            .any(|prefix| hostname.starts_with(prefix))
        || INTERNAL_SUFFIXES
            .iter()
            .any(|suffix| hostname.ends_with(suffix))
        || is_172_private_prefix(hostname)
}

/// `^172\.(1[6-9]|2\d|3[01])\.`
fn is_172_private_prefix(hostname: &str) -> bool {
    let Some(rest) = hostname.strip_prefix("172.") else {
        return false;
    };
    let Some((octet, _)) = rest.split_once('.') else {
        return false;
    };
    octet.len() == 2
        && octet
            .parse::<u8>()
            .is_ok_and(|value| (16..=31).contains(&value))
}

/// `isPrivateIP`: loopback, unspecified, RFC 1918, link-local and unique-local
/// addresses, including IPv4 addresses mapped into IPv6.
pub fn is_private_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_unspecified() || v4.is_private() || v4.is_link_local()
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || first & 0xffc0 == 0xfe80
                || first & 0xfe00 == 0xfc00
        }
    }
}

/// The address a host names directly, without brackets.
pub fn ip_literal(host: &str) -> Option<IpAddr> {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .ok()
}
