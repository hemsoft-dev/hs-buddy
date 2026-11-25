//! A bookmark's favicon, as Electron's bookmark cards and sidebar rows load
//! `faviconUrl` into an `<img>`. The sidebar's `isSafeImageUrl` is the rule for
//! both here: http(s) only and no internal host. The fetch is the page title's:
//! the host is resolved and checked, the request goes only to those addresses,
//! every redirect is checked again, and each request has a 5 second timeout.
//!
//! The body is capped at [`MAX_BYTES`] and must be a PNG, JPEG, GIF, WebP, ICO
//! or BMP image, recognised from its first bytes. It is decoded here, never by
//! GPUI: the decoder refuses an image over [`MAX_SIDE`] pixels a side or one
//! that needs more than [`MAX_DECODE_BYTES`], takes only the first frame of an
//! animation, and the result is scaled down to at most [`ICON_SIDE`] a side.
//! GPUI is handed those few pixels, so a favicon holds at most 16 KiB however
//! its file is built. SVG is refused: it isn't pixels, and drawing one would
//! need a renderer outside these limits.
use std::io::Cursor;

use image::imageops::FilterType;
use image::{ImageError, ImageFormat, ImageReader, Limits, RgbaImage};
use reqwest::Url;
use reqwest::header::{ACCEPT, CONTENT_LENGTH, LOCATION};

use super::title::{Resolver, TitleError, pinned_client, public_resolver};
use super::url::{UrlError, validate_url};
use crate::browser::FixtureOrigin;

/// A favicon larger than this is refused, unread past the limit.
pub const MAX_BYTES: usize = 256 * 1024;
/// The widest or tallest image the decoder accepts.
pub const MAX_SIDE: u32 = 512;
/// What one decode may allocate, its output included: a 512 px RGBA image at
/// 16 bits a channel is 2 MiB, which leaves the decoder room for its own buffers.
pub const MAX_DECODE_BYTES: u64 = 4 * 1024 * 1024;
/// The largest favicon kept: a 20 px card icon at 3x, with room to spare.
pub const ICON_SIDE: u32 = 64;
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

/// The image formats a favicon may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Webp,
    Ico,
    Bmp,
}

impl Format {
    fn decoder(self) -> ImageFormat {
        match self {
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Gif => ImageFormat::Gif,
            Self::Webp => ImageFormat::WebP,
            Self::Ico => ImageFormat::Ico,
            Self::Bmp => ImageFormat::Bmp,
        }
    }
}

/// A decoded favicon: one frame of RGBA pixels, at most [`ICON_SIDE`] a side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Favicon {
    pub format: Format,
    pub image: RgbaImage,
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
        return sanitize(&bytes);
    }
    Err(FaviconError::TooManyRedirects)
}

/// The first frame of a supported image no larger than [`MAX_SIDE`] a side,
/// decoded within [`MAX_DECODE_BYTES`] and scaled to fit [`ICON_SIDE`].
///
/// The decoder checks the size it is about to produce before it allocates, so
/// the limits hold for every format: an ICO is decoded from the entry the
/// decoder picks, whatever its directory says, and a GIF from its first frame.
pub fn sanitize(bytes: &[u8]) -> Result<Favicon, FaviconError> {
    let format = sniff(bytes).ok_or(FaviconError::NotAnImage)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format.decoder());
    reader.limits(limits);
    let decoded = reader.decode().map_err(|error| match error {
        ImageError::Limits(_) => FaviconError::TooLarge,
        _ => FaviconError::NotAnImage,
    })?;
    if decoded.width() == 0 || decoded.height() == 0 {
        return Err(FaviconError::NotAnImage);
    }
    let fitted = if decoded.width() > ICON_SIDE || decoded.height() > ICON_SIDE {
        // Keeps the aspect ratio; the result fits inside ICON_SIDE a side.
        decoded.resize(ICON_SIDE, ICON_SIDE, FilterType::Triangle)
    } else {
        decoded
    };
    Ok(Favicon {
        format,
        image: fitted.into_rgba8(),
    })
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
