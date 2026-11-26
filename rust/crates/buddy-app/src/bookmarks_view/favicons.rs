//! Bookmark favicons, kept in memory for the session per `faviconUrl`, as
//! Electron's `<img>` tags load them. Nothing is saved to disk: Electron keeps
//! no favicon store of its own either.
//!
//! A favicon is fetched once per address, at most [`IN_FLIGHT`] at a time, and
//! never for anything but an http(s) address. Until one arrives, and when it
//! fails, the bookmark shows its globe icon.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use buddy_core::bookmarks::favicon::Favicon;
use gpui_kit::RenderImage;

/// Favicon requests running at once; the rest wait their turn.
pub(super) const IN_FLIGHT: usize = 4;

enum State {
    Waiting,
    Loading,
    Ready(Arc<RenderImage>),
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

/// The already decoded pixels as GPUI's single frame, in the BGRA order it
/// draws, so GPUI never decodes a fetched file itself.
fn render_image(favicon: Favicon) -> Arc<RenderImage> {
    let mut pixels = favicon.image;
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new([image::Frame::new(pixels)]))
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

    /// Forgets every address no bookmark uses any more (deleted, or its
    /// `faviconUrl` changed), and returns the images to release from GPUI.
    /// A fetch still running is kept until it finishes, so it is never
    /// started twice; the next call forgets it.
    pub fn retain<'a>(&mut self, urls: impl IntoIterator<Item = &'a str>) -> Vec<Arc<RenderImage>> {
        let keep: HashSet<&str> = urls.into_iter().collect();
        self.queue.retain(|url| keep.contains(url.as_str()));
        let gone: Vec<String> = self
            .states
            .iter()
            .filter(|(url, state)| !keep.contains(url.as_str()) && !matches!(state, State::Loading))
            .map(|(url, _)| url.clone())
            .collect();
        gone.into_iter()
            .filter_map(|url| match self.states.remove(&url) {
                Some(State::Ready(image)) => Some(image),
                _ => None,
            })
            .collect()
    }

    /// Records a finished fetch, and returns the next ones to start.
    pub fn finished(&mut self, url: &str, favicon: Option<Favicon>) -> Vec<String> {
        self.running = self.running.saturating_sub(1);
        let state = match favicon {
            Some(favicon) => State::Ready(render_image(favicon)),
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
    pub fn image(&self, url: &str) -> Option<Arc<RenderImage>> {
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

    #[cfg(test)]
    pub fn known(&self) -> usize {
        self.states.len()
    }
}

#[cfg(test)]
mod tests {
    use buddy_core::bookmarks::favicon::Format;

    use super::*;

    fn icon() -> Favicon {
        Favicon {
            format: Format::Png,
            image: image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 128, 255])),
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

    #[test]
    fn the_image_is_handed_over_decoded_in_gpui_order() {
        let mut favicons = Favicons::default();
        let url = "https://e.test/i.png";
        favicons.want([url]);
        favicons.finished(url, Some(icon()));
        let image = favicons.image(url).unwrap();
        assert_eq!(image.frame_count(), 1);
        assert_eq!(&image.as_bytes(0).unwrap()[..4], &[128, 0, 255, 255]);
    }

    #[test]
    fn addresses_no_bookmark_uses_are_forgotten_and_their_images_released() {
        let mut favicons = Favicons::default();
        let urls: Vec<String> = (0..6).map(|n| format!("https://e{n}.test/i.png")).collect();
        favicons.want(urls.iter().map(String::as_str));
        favicons.finished(&urls[0], Some(icon()));
        favicons.finished(&urls[1], None);
        favicons.finished(&urls[2], Some(icon()));
        // 0 and 2 are loaded, 1 failed, and 3, 4 and 5 are loading.
        let kept = [urls[2].as_str(), urls[5].as_str()];
        let released = favicons.retain(kept);
        assert_eq!(released.len(), 1, "only the loaded image no bookmark uses");
        assert!(favicons.image(&urls[0]).is_none());
        assert!(
            !favicons.failed(&urls[1]),
            "a forgotten failure may be tried again"
        );
        assert!(favicons.image(&urls[2]).is_some());
        // Loading fetches finish, and the next call forgets those no bookmark uses.
        for url in &urls[3..] {
            favicons.finished(url, Some(icon()));
        }
        assert_eq!(favicons.retain(kept).len(), 2);
        assert_eq!(favicons.known(), 2);
        assert!(favicons.retain([]).len() == 2 && favicons.known() == 0);
        // An address used again is fetched again.
        assert_eq!(favicons.want([urls[0].as_str()]), vec![urls[0].clone()]);
    }
}
