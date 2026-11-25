//! A bookmark's favicon, as Electron's bookmark cards and sidebar rows load
//! `faviconUrl` into an `<img>`. The sidebar's `isSafeImageUrl` is the rule for
//! both here: http(s) only and no internal host. The fetch is the page title's:
//! the host is resolved and checked, the request goes only to those addresses,
//! every redirect is checked again, and each request has a 5 second timeout.
//!
//! The body is capped at [`MAX_BYTES`] and must be a PNG, JPEG, GIF, ICO or BMP
//! image, recognised from its first bytes. It is decoded here, never by
//! GPUI: the decoder refuses an image over [`MAX_SIDE`] pixels a side or one
//! that needs more than [`MAX_DECODE_BYTES`], takes only the first frame of an
//! animation, and the result is scaled down to at most [`ICON_SIDE`] a side.
//! GPUI is handed those few pixels, so a favicon holds at most 16 KiB however
//! its file is built. SVG is refused: it isn't pixels, and drawing one would
//! need a renderer outside these limits. WebP is refused before decoding: the
//! WebP decoder (`image-webp` 0.2) decodes a lossy frame larger than the canvas
//! it declares before comparing the two, so the limits don't bound it.
//!
//! The PNG decoder inflates a compressed colour profile or text chunk while it
//! reads the header, and the ICO decoder reads an embedded PNG with no limits
//! at all. So a PNG keeps only the chunks it needs before it is decoded, and
//! an icon's directory is read here: its PNG entry is filtered the same way,
//! and the ICO decoder sees only a single bitmap entry.
use std::io::Cursor;

use image::imageops::FilterType;
use image::{DynamicImage, ImageError, ImageFormat, ImageReader, Limits, RgbaImage};
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
    Ico,
    Bmp,
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
/// The decoders check the size they are about to produce before they
/// allocate. A PNG reaches its decoder only through [`filter_png`], because
/// the PNG decoder inflates compressed colour profiles and text while it reads
/// the header. An ICO is never handed to the ICO decoder whole: [`decode_icon`]
/// reads its directory, and the one entry it picks is decoded on its own.
pub fn sanitize(bytes: &[u8]) -> Result<Favicon, FaviconError> {
    let format = sniff(bytes).ok_or(FaviconError::NotAnImage)?;
    let decoded = match format {
        Format::Png => decode_png(bytes)?,
        Format::Ico => decode_icon(bytes)?,
        Format::Jpeg => decode(bytes, ImageFormat::Jpeg)?,
        Format::Gif => decode(bytes, ImageFormat::Gif)?,
        Format::Bmp => decode(bytes, ImageFormat::Bmp)?,
    };
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

/// One decode of `bytes` as `format`, within the limits.
fn decode(bytes: &[u8], format: ImageFormat) -> Result<DynamicImage, FaviconError> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    reader.decode().map_err(|error| match error {
        ImageError::Limits(_) => FaviconError::TooLarge,
        _ => FaviconError::NotAnImage,
    })
}

fn decode_png(bytes: &[u8]) -> Result<DynamicImage, FaviconError> {
    decode(&filter_png(bytes)?, ImageFormat::Png)
}

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
/// The chunks a favicon needs. Every other chunk is ancillary metadata (colour
/// profiles, text, gamma, animation control) and is dropped.
const PNG_KEPT_CHUNKS: [&[u8; 4]; 5] = [b"IHDR", b"PLTE", b"tRNS", b"IDAT", b"IEND"];

/// The PNG `bytes` with only [`PNG_KEPT_CHUNKS`], each copied as it is.
///
/// Every chunk's length must fit in what is left of the input and its CRC must
/// match. The first chunk must be a 13 byte IHDR, there must be an IDAT, and
/// the file ends at IEND (anything after it is dropped). A critical chunk the
/// favicon doesn't use, or a chunk type that isn't four letters, makes the file
/// unreadable, as the PNG specification says. The PNG decoder therefore never
/// sees a compressed chunk (iCCP, zTXt, iTXt) it would inflate while reading
/// the header; only IDAT is inflated, into the output the limits allowed. An
/// animated PNG becomes its static image.
pub fn filter_png(bytes: &[u8]) -> Result<Vec<u8>, FaviconError> {
    let mut rest = bytes
        .strip_prefix(PNG_SIGNATURE)
        .ok_or(FaviconError::NotAnImage)?;
    let mut filtered = Vec::with_capacity(bytes.len());
    filtered.extend_from_slice(PNG_SIGNATURE);
    let (mut first, mut has_idat) = (true, false);
    loop {
        if rest.len() < 12 {
            return Err(FaviconError::NotAnImage);
        }
        let length = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]);
        let length = usize::try_from(length).map_err(|_| FaviconError::NotAnImage)?;
        if length > (i32::MAX as usize) || length > rest.len() - 12 {
            return Err(FaviconError::NotAnImage);
        }
        let kind = [rest[4], rest[5], rest[6], rest[7]];
        let crc_at = 8 + length;
        let crc = u32::from_be_bytes([
            rest[crc_at],
            rest[crc_at + 1],
            rest[crc_at + 2],
            rest[crc_at + 3],
        ]);
        if !kind.iter().all(u8::is_ascii_alphabetic) || crc32(&rest[4..crc_at]) != crc {
            return Err(FaviconError::NotAnImage);
        }
        let (chunk, after) = rest.split_at(crc_at + 4);
        rest = after;
        let is_ihdr = &kind == b"IHDR";
        if first != is_ihdr || (is_ihdr && length != 13) {
            return Err(FaviconError::NotAnImage);
        }
        first = false;
        if PNG_KEPT_CHUNKS.contains(&&kind) {
            filtered.extend_from_slice(chunk);
        } else if kind[0].is_ascii_uppercase() {
            return Err(FaviconError::NotAnImage);
        }
        has_idat |= &kind == b"IDAT";
        if &kind == b"IEND" {
            break;
        }
    }
    if !has_idat {
        return Err(FaviconError::NotAnImage);
    }
    Ok(filtered)
}

/// PNG's CRC-32 (ISO 3309, reflected, polynomial 0xEDB88320).
fn crc32(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut n = 0;
        while n < 256 {
            let mut crc = n as u32;
            let mut bit = 0;
            while bit < 8 {
                crc = if crc & 1 == 1 {
                    0xEDB8_8320 ^ (crc >> 1)
                } else {
                    crc >> 1
                };
                bit += 1;
            }
            table[n] = crc;
            n += 1;
        }
        table
    };
    !bytes.iter().fold(!0u32, |crc, byte| {
        TABLE[((crc ^ u32::from(*byte)) & 0xFF) as usize] ^ (crc >> 8)
    })
}

/// An icon directory holds a few sizes of one icon; more than this is refused.
pub const MAX_ICON_ENTRIES: usize = 256;
const ICON_DIRECTORY: usize = 6;
const ICON_ENTRY: usize = 16;

/// One entry of an icon directory, its image found inside the file.
struct IconEntry<'a> {
    raw: &'a [u8],
    image: &'a [u8],
    width: u32,
    height: u32,
    bits: u16,
    png: bool,
}

/// The best entry of an icon: the one with the most bits a pixel, then the
/// most pixels, of those no larger than [`MAX_SIDE`] a side.
///
/// The directory is read here. Its count must be 1 to [`MAX_ICON_ENTRIES`],
/// and every entry's image must lie inside the file. A PNG entry's size is
/// read from its IHDR, a bitmap's from the directory (at most 256 px). A PNG
/// entry goes through [`filter_png`] like any PNG. A bitmap entry is decoded
/// alone, from a one-entry icon built around it, so the ICO decoder never
/// meets a PNG.
fn decode_icon(bytes: &[u8]) -> Result<DynamicImage, FaviconError> {
    let count = bytes
        .get(4..ICON_DIRECTORY)
        .map(|count| usize::from(u16::from_le_bytes([count[0], count[1]])))
        .filter(|count| (1..=MAX_ICON_ENTRIES).contains(count))
        .ok_or(FaviconError::NotAnImage)?;
    let directory = bytes
        .get(ICON_DIRECTORY..ICON_DIRECTORY + count * ICON_ENTRY)
        .ok_or(FaviconError::NotAnImage)?;
    let mut best: Option<IconEntry> = None;
    let mut too_large = false;
    for raw in directory.as_chunks::<ICON_ENTRY>().0 {
        let field =
            |at: usize| u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
        let (length, offset) = (field(8) as usize, field(12) as usize);
        let image = offset
            .checked_add(length)
            .and_then(|end| bytes.get(offset..end))
            .filter(|image| !image.is_empty())
            .ok_or(FaviconError::NotAnImage)?;
        let png = image.starts_with(PNG_SIGNATURE);
        let (width, height) = if png {
            match png_size(image) {
                Some(size) => size,
                None => continue,
            }
        } else {
            let side = |byte: u8| if byte == 0 { 256 } else { u32::from(byte) };
            (side(raw[0]), side(raw[1]))
        };
        if width > MAX_SIDE || height > MAX_SIDE {
            too_large = true;
            continue;
        }
        let entry = IconEntry {
            raw,
            image,
            width,
            height,
            bits: u16::from_le_bytes([raw[6], raw[7]]),
            png,
        };
        let score = |entry: &IconEntry| (entry.bits, entry.width * entry.height);
        if best.as_ref().is_none_or(|best| score(&entry) > score(best)) {
            best = Some(entry);
        }
    }
    let Some(entry) = best else {
        return Err(if too_large {
            FaviconError::TooLarge
        } else {
            FaviconError::NotAnImage
        });
    };
    if entry.png {
        return decode_png(entry.image);
    }
    let image_at = ICON_DIRECTORY + ICON_ENTRY;
    let mut single = Vec::with_capacity(image_at + entry.image.len());
    single.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    single.extend_from_slice(&entry.raw[..12]);
    single.extend_from_slice(&(image_at as u32).to_le_bytes());
    single.extend_from_slice(entry.image);
    decode(&single, ImageFormat::Ico)
}

/// The width and height an embedded PNG's IHDR declares.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let header = png.get(8..24)?;
    if header[..8] != [0, 0, 0, 13, b'I', b'H', b'D', b'R'] {
        return None;
    }
    let width = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
    let height = u32::from_be_bytes([header[12], header[13], header[14], header[15]]);
    (width > 0 && height > 0).then_some((width, height))
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
/// A WebP file (`RIFF....WEBP`) is not one of them, so it is never decoded.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Format::Gif)
    } else if bytes.starts_with(&[0, 0, 1, 0]) && bytes.len() >= 6 {
        Some(Format::Ico)
    } else if bytes.starts_with(b"BM") && bytes.len() >= 26 {
        Some(Format::Bmp)
    } else {
        None
    }
}
