//! Bookmark favicons, kept in memory for the session per `faviconUrl`, as
//! Electron's `<img>` tags load them. Nothing is saved to disk: Electron keeps
//! no favicon store of its own either.
//!
//! A favicon is fetched once per address, at most [`IN_FLIGHT`] at a time, and
//! never for anything but an http(s) address. Until one arrives, and when it
//! fails, the bookmark shows its globe icon.
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use buddy_core::bookmarks::favicon::{Favicon, Format};
use gpui_kit::{Image, ImageFormat};

/// Favicon requests running at once; the rest wait their turn.
pub(super) const IN_FLIGHT: usize = 4;

enum State {
    Waiting,
    Loading,
    Ready(Arc<Image>),
    Failed,
}

#[derive(Default)]
pub(super) struct Favicons {
    states: HashMap<String, State>,
    queue: VecDeque<String>,
    running: usize,
}

/// Whether the address is one a favicon may be fetched from at all; the fetch
/// itself applies the full check.
fn fetchable(url: &str) -> bool {
    buddy_core::browser::Url::parse(url.trim())
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

fn image_format(format: Format) -> ImageFormat {
    match format {
        Format::Png => ImageFormat::Png,
        Format::Jpeg => ImageFormat::Jpeg,
        Format::Gif => ImageFormat::Gif,
        Format::Webp => ImageFormat::Webp,
        Format::Ico => ImageFormat::Ico,
        Format::Bmp => ImageFormat::Bmp,
    }
}

impl Favicons {
    /// Queues every address not seen before, and returns the ones to fetch now.
    pub fn want<'a>(&mut self, urls: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        for url in urls {
            if !self.states.contains_key(url) && fetchable(url) {
                self.states.insert(url.to_string(), State::Waiting);
                self.queue.push_back(url.to_string());
            }
        }
        self.start()
    }

    /// Records a finished fetch, and returns the next ones to start.
    pub fn finished(&mut self, url: &str, favicon: Option<Favicon>) -> Vec<String> {
        self.running = self.running.saturating_sub(1);
        let state = match favicon {
            Some(favicon) => State::Ready(Arc::new(Image::from_bytes(
                image_format(favicon.format),
                favicon.bytes,
            ))),
            None => State::Failed,
        };
        self.states.insert(url.to_string(), state);
        self.start()
    }

    fn start(&mut self) -> Vec<String> {
        let mut started = Vec::new();
        while self.running < IN_FLIGHT
            && let Some(url) = self.queue.pop_front()
        {
            self.states.insert(url.clone(), State::Loading);
            self.running += 1;
            started.push(url);
        }
        started
    }

    /// The loaded image for `url`, if it has arrived.
    pub fn image(&self, url: &str) -> Option<Arc<Image>> {
        match self.states.get(url) {
            Some(State::Ready(image)) => Some(image.clone()),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn failed(&self, url: &str) -> bool {
        matches!(self.states.get(url), Some(State::Failed))
    }

    #[cfg(test)]
    pub fn pending(&self) -> usize {
        self.states
            .values()
            .filter(|state| matches!(state, State::Waiting | State::Loading))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon() -> Favicon {
        Favicon {
            format: Format::Png,
            bytes: vec![1, 2, 3],
        }
    }

    #[test]
    fn each_address_is_fetched_once_and_a_few_at_a_time() {
        let mut favicons = Favicons::default();
        let urls: Vec<String> = (0..6).map(|n| format!("https://e{n}.test/i.png")).collect();
        let started = favicons.want(urls.iter().map(String::as_str));
        assert_eq!(started, urls[..IN_FLIGHT].to_vec());
        // Asking again, as each poll does, starts nothing new.
        assert!(favicons.want(urls.iter().map(String::as_str)).is_empty());
        assert_eq!(
            favicons.finished(&urls[0], Some(icon())),
            vec![urls[4].clone()]
        );
        assert_eq!(favicons.finished(&urls[1], None), vec![urls[5].clone()]);
        assert!(favicons.image(&urls[0]).is_some());
        assert!(favicons.image(&urls[1]).is_none() && favicons.failed(&urls[1]));
        // A failure isn't retried within the session.
        assert!(favicons.want([urls[1].as_str()]).is_empty());
        for url in &urls[2..] {
            assert!(favicons.finished(url, Some(icon())).is_empty());
        }
        assert_eq!(favicons.pending(), 0);
    }

    #[test]
    fn only_http_addresses_are_fetched() {
        let mut favicons = Favicons::default();
        let started = favicons.want([
            "javascript:alert(1)",
            "data:image/png;base64,AAAA",
            "file:///etc/hosts",
            "ftp://example.com/i.png",
            "",
            "not a url",
            "http://example.com/i.png",
        ]);
        assert_eq!(started, vec!["http://example.com/i.png".to_string()]);
        assert_eq!(favicons.pending(), 1);
    }
}
