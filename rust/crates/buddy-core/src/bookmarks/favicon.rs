//! A bookmark's favicon, as Electron's bookmark cards and sidebar rows load
//! `faviconUrl` into an `<img>`. The sidebar's `isSafeImageUrl` is the rule for
//! both here: http(s) only and no internal host. The fetch is the page title's:
//! the host is resolved and checked, the request goes only to those addresses,
//! every redirect is checked again, and each request has a 5 second timeout.
//!
//! The body is capped at [`MAX_BYTES`] and must be a PNG, JPEG, GIF, WebP, ICO
//! or BMP image, recognised from its first bytes, whose declared size is at most
//! [`MAX_SIDE`] pixels a side. SVG is refused: GPUI draws an SVG at its declared
//! size, so a tiny file could ask for an enormous bitmap.
use reqwest::Url;
use reqwest::header::{ACCEPT, CONTENT_LENGTH, LOCATION};

use super::title::{Resolver, TitleError, pinned_client, public_resolver};
use super::url::{UrlError, validate_url};
use crate::browser::FixtureOrigin;

/// A favicon larger than this is refused, unread past the limit.
pub const MAX_BYTES: usize = 256 * 1024;
/// The widest or tallest image accepted, so decoding stays small.
pub const MAX_SIDE: u32 = 1024;
const MAX_REDIRECTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FaviconError {
    #[error("{0}")]
    Url(#[from] UrlError),
    #[error("The host name could not be resolved")]
    Dns,
    #[error("The image could not be reached")]
    Network,
    #[error("HTTP {0}")]
    Status(u16),
    #[error("Too many redirects")]
    TooManyRedirects,
    #[error("The image is too large")]
    TooLarge,
    #[error("Not a supported image")]
    NotAnImage,
}

impl From<TitleError> for FaviconError {
    fn from(error: TitleError) -> Self {
        match error {
            TitleError::Url(error) => Self::Url(error),
            TitleError::Dns => Self::Dns,
            _ => Self::Network,
        }
    }
}

/// The image formats a favicon may be, all of which GPUI decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Webp,
    Ico,
    Bmp,
}

/// A downloaded favicon, checked but not decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Favicon {
    pub format: Format,
    pub bytes: Vec<u8>,
}

/// The address to load, or `None` when the bookmark's `faviconUrl` may not be
/// fetched. The qualification fixture's exact loopback origin is admitted, as
/// the embedded browser admits it.
pub fn target(raw: &str, fixture: Option<&FixtureOrigin>) -> Option<Url> {
    checked(raw.trim(), fixture).ok()
}

fn checked(raw: &str, fixture: Option<&FixtureOrigin>) -> Result<Url, UrlError> {
    if let Some(origin) = fixture
        && let Ok(url) = Url::parse(raw)
        && origin.admits(&url)
    {
        return Ok(url);
    }
    validate_url(raw)
}

pub async fn fetch_favicon(
    url: &str,
    fixture: Option<&FixtureOrigin>,
) -> Result<Favicon, FaviconError> {
    fetch_favicon_with(url, fixture, &public_resolver()).await
}

pub async fn fetch_favicon_with(
    url: &str,
    fixture: Option<&FixtureOrigin>,
    resolver: &Resolver,
) -> Result<Favicon, FaviconError> {
    let mut current = checked(url.trim(), fixture)?;
    for _ in 0..=MAX_REDIRECTS {
        let client = pinned_client(&current, resolver).await?;
        let response = client
            .get(current.clone())
            .header(ACCEPT, "image/*")
            .send()
            .await
            .map_err(|_| FaviconError::Network)?;
        let status = response.status();
        if status.is_redirection() && status.as_u16() != 304 {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(FaviconError::Network)?;
            let next = current.join(location).map_err(|_| UrlError::Invalid)?;
            current = checked(next.as_str(), fixture)?;
            continue;
        }
        if !status.is_success() {
            return Err(FaviconError::Status(status.as_u16()));
        }
        let bytes = read_capped(response).await?;
        return check_image(bytes);
    }
    Err(FaviconError::TooManyRedirects)
}

/// A body that is a supported image no larger than [`MAX_SIDE`] a side.
pub fn check_image(bytes: Vec<u8>) -> Result<Favicon, FaviconError> {
    let format = sniff(&bytes).ok_or(FaviconError::NotAnImage)?;
    match dimensions(format, &bytes) {
        Some((width, height)) if width > MAX_SIDE || height > MAX_SIDE => {
            Err(FaviconError::TooLarge)
        }
        Some((width, height)) if width > 0 && height > 0 => Ok(Favicon { format, bytes }),
        _ => Err(FaviconError::NotAnImage),
    }
}

/// The whole body, refused once it passes [`MAX_BYTES`].
async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>, FaviconError> {
    let declared = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > MAX_BYTES) {
        return Err(FaviconError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| FaviconError::Network)? {
        if body.len() + chunk.len() > MAX_BYTES {
            return Err(FaviconError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// The format from the file's signature; a server's content type is often wrong
/// for favicons (`text/plain`, `application/octet-stream`), so it isn't used.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Format::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(Format::Webp)
    } else if bytes.starts_with(&[0, 0, 1, 0]) && bytes.len() >= 6 {
        Some(Format::Ico)
    } else if bytes.starts_with(b"BM") && bytes.len() >= 26 {
        Some(Format::Bmp)
    } else {
        None
    }
}

/// The declared width and height, or `None` when the header can't be read.
pub fn dimensions(format: Format, bytes: &[u8]) -> Option<(u32, u32)> {
    let le16 = |at: usize| {
        Some(u32::from(u16::from_le_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    let be32 = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let le32 = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    // BMP stores a negative height for top-down rows.
    let signed = |at: usize| {
        Some(i32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?).unsigned_abs())
    };
    let le24 = |at: usize| {
        let b = bytes.get(at..at + 3)?;
        Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
    };
    match format {
        // IHDR is always the first chunk.
        Format::Png => Some((be32(16)?, be32(20)?)),
        Format::Gif => Some((le16(6)?, le16(8)?)),
        Format::Bmp => Some((signed(18)?, signed(22)?)),
        Format::Jpeg => jpeg_dimensions(bytes),
        Format::Webp => match bytes.get(12..16)? {
            b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
            b"VP8L" => {
                let bits = le32(21)?;
                Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
            }
            b"VP8 " => Some((le16(26)? & 0x3FFF, le16(28)? & 0x3FFF)),
            _ => None,
        },
        // The largest entry; an entry may hold a PNG of any size.
        Format::Ico => {
            let count = le16(4)? as usize;
            let mut largest = (0, 0);
            for entry in 0..count.min(64) {
                let at = 6 + entry * 16;
                let side = |byte: u8| if byte == 0 { 256 } else { u32::from(byte) };
                let mut size = (side(*bytes.get(at)?), side(*bytes.get(at + 1)?));
                let image = bytes.get(usize::try_from(le32(at + 12)?).ok()?..)?;
                if sniff(image) == Some(Format::Png) {
                    size = dimensions(Format::Png, image)?;
                }
                largest = (largest.0.max(size.0), largest.1.max(size.1));
            }
            (count > 0).then_some(largest)
        }
    }
}

/// The first start-of-frame marker's height and width.
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        if marker == 0xFF {
            at += 1;
            continue;
        }
        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        let frame = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if frame {
            let header = bytes.get(at + 5..at + 9)?;
            let height = u16::from_be_bytes([header[0], header[1]]) as u32;
            let width = u16::from_be_bytes([header[2], header[3]]) as u32;
            return Some((width, height));
        }
        at += 2 + length;
    }
    None
}
