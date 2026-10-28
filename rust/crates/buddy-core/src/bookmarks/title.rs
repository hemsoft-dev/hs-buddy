//! A page's title for a pasted URL, as Electron's `shell:fetch-page-title` reads it:
//! http(s) only, no internal hosts, every redirect checked again, a 5 second timeout
//! per request and at most 64 KiB of HTML.
//!
//! The host is resolved here and each address checked, and the request then connects
//! to exactly those addresses, so a name cannot resolve to a public address for the
//! check and a private one for the request.
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use reqwest::Url;
use reqwest::header::{ACCEPT, CONTENT_TYPE, LOCATION};

use super::url::{UrlError, ip_literal, is_private_ip, validate_url};

pub const TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_BODY: usize = 64 * 1024;
const MAX_REDIRECTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TitleError {
    #[error("{0}")]
    Url(#[from] UrlError),
    #[error("The host name could not be resolved")]
    Dns,
    #[error("The page could not be reached")]
    Network,
    #[error("HTTP {0}")]
    Status(u16),
    #[error("Not an HTML page")]
    NotHtml,
    #[error("Too many redirects")]
    TooManyRedirects,
    #[error("No title found")]
    NoTitle,
}

/// Turns a host and port into the addresses a request may use, or refuses them.
pub type Resolver = Arc<
    dyn Fn(String, u16) -> BoxFuture<'static, Result<Vec<SocketAddr>, TitleError>> + Send + Sync,
>;

/// The system resolver, refusing a name with any private or loopback address.
pub fn public_resolver() -> Resolver {
    Arc::new(|host, port| {
        Box::pin(async move {
            let addresses: Vec<SocketAddr> =
                tokio::time::timeout(TIMEOUT, tokio::net::lookup_host((host.as_str(), port)))
                    .await
                    .map_err(|_| TitleError::Dns)?
                    .map_err(|_| TitleError::Dns)?
                    .collect();
            check_addresses(&addresses)?;
            Ok(addresses)
        })
    })
}

/// Electron's `assertNoPrivateIPs`: one private address refuses the name.
pub fn check_addresses(addresses: &[SocketAddr]) -> Result<(), TitleError> {
    if addresses.is_empty() {
        return Err(TitleError::Dns);
    }
    if addresses.iter().any(|address| is_private_ip(address.ip())) {
        return Err(UrlError::Internal.into());
    }
    Ok(())
}

pub async fn fetch_page_title(url: &str) -> Result<String, TitleError> {
    fetch_page_title_with(url, &public_resolver()).await
}

pub async fn fetch_page_title_with(url: &str, resolver: &Resolver) -> Result<String, TitleError> {
    let html = fetch_html(url, resolver).await?;
    extract_page_title(&html).ok_or(TitleError::NoTitle)
}

async fn fetch_html(url: &str, resolver: &Resolver) -> Result<String, TitleError> {
    let mut current = validate_url(url)?;
    for _ in 0..=MAX_REDIRECTS {
        let client = pinned_client(&current, resolver).await?;
        let response = client
            .get(current.clone())
            .header(ACCEPT, "text/html")
            .send()
            .await
            .map_err(|_| TitleError::Network)?;
        let status = response.status();
        if status.is_redirection() && status.as_u16() != 304 {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(TitleError::Network)?;
            let next = current.join(location).map_err(|_| UrlError::Invalid)?;
            current = validate_url(next.as_str())?;
            continue;
        }
        if !status.is_success() {
            return Err(TitleError::Status(status.as_u16()));
        }
        return read_html(response).await;
    }
    Err(TitleError::TooManyRedirects)
}

/// A client that connects only to the checked addresses of `url`'s host, through no
/// proxy (a proxy would resolve the name itself) and without following redirects.
async fn pinned_client(url: &Url, resolver: &Resolver) -> Result<reqwest::Client, TitleError> {
    let host = url.host_str().ok_or(UrlError::Invalid)?.to_string();
    let port = url.port_or_known_default().ok_or(UrlError::Invalid)?;
    let builder = reqwest::Client::builder()
        .user_agent(crate::http::USER_AGENT)
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    let builder = if ip_literal(&host).is_some() {
        // validate_url refused private literals; nothing to resolve.
        builder
    } else {
        let addresses = resolver(host.clone(), port).await?;
        builder.resolve_to_addrs(&host, &addresses)
    };
    builder.build().map_err(|_| TitleError::Network)
}

async fn read_html(mut response: reqwest::Response) -> Result<String, TitleError> {
    let html = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/html"));
    if !html {
        return Err(TitleError::NotHtml);
    }
    let mut body = Vec::new();
    while body.len() < MAX_BODY {
        match response.chunk().await.map_err(|_| TitleError::Network)? {
            Some(chunk) => body.extend_from_slice(&chunk),
            None => break,
        }
    }
    body.truncate(MAX_BODY);
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// `extractPageTitle`: the `<title>` text, else `og:title`, entities decoded.
pub fn extract_page_title(html: &str) -> Option<String> {
    extract_title_tag(html)
        .filter(|title| !title.is_empty())
        .or_else(|| extract_og_title(html))
        .map(|title| decode_html_entities(&title))
}

fn extract_title_tag(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title>")?;
    if end == start {
        return None;
    }
    Some(
        html[start..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn extract_og_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find("<meta") {
        let start = from + found;
        let end = start + lower[start..].find('>').unwrap_or(lower.len() - start);
        let tag = &html[start..end];
        if attribute(tag, "property").is_some_and(|value| value.eq_ignore_ascii_case("og:title"))
            && let Some(content) = attribute(tag, "content").map(str::trim)
            && !content.is_empty()
        {
            return Some(content.to_string());
        }
        from = end;
    }
    None
}

/// A quoted attribute's value in one tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let after = from + found + name.len();
        let preceded = lower[..from + found]
            .chars()
            .last()
            .is_some_and(char::is_whitespace);
        let rest = &tag[after..];
        if preceded && let Some(rest) = rest.strip_prefix('=') {
            let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
            let value = &rest[1..];
            return value.find(quote).map(|end| &value[..end]);
        }
        from = after;
    }
    None
}

/// `decodeHtmlEntities`: the five XML entities, `&nbsp;` and numeric references.
pub fn decode_html_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        match decode_one(rest) {
            Some((decoded, length)) => {
                out.push_str(&decoded);
                rest = &rest[length..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_one(text: &str) -> Option<(String, usize)> {
    let end = text.find(';')?;
    let body = &text[1..end];
    let decoded = match body {
        "amp" => "&".to_string(),
        "lt" => "<".to_string(),
        "gt" => ">".to_string(),
        "quot" => "\"".to_string(),
        "apos" => "'".to_string(),
        "nbsp" => " ".to_string(),
        _ => {
            let number = body.strip_prefix('#')?;
            let value = match number.strip_prefix(['x', 'X']) {
                Some(hex) if !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()) => {
                    u32::from_str_radix(hex, 16).unwrap_or(u32::MAX)
                }
                None if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) => {
                    number.parse().unwrap_or(u32::MAX)
                }
                _ => return None,
            };
            let character = (value != 0)
                .then(|| char::from_u32(value))
                .flatten()
                .unwrap_or('\u{FFFD}');
            character.to_string()
        }
    };
    Some((decoded, end + 1))
}
