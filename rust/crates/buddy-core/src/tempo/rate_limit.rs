//! Every request on a captured client shares pacing; reads alone may retry 429.
use super::TransportError;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

const SPACING: Duration = Duration::from_secs(1);
const MAX_WAIT: Duration = Duration::from_secs(30);
struct State {
    next: Instant,
    blocked: bool,
}
pub(super) struct Pacer(Mutex<State>);
impl Pacer {
    pub(super) fn new() -> Self {
        Self(Mutex::new(State {
            next: Instant::now(),
            blocked: false,
        }))
    }
    pub(super) async fn wait(&self) -> Result<(), TransportError> {
        let mut state = self.0.lock().await;
        if state.blocked || state.next.saturating_duration_since(Instant::now()) > MAX_WAIT {
            return Err(TransportError::RateLimited);
        }
        tokio::time::sleep_until(state.next).await;
        // Reserve from the actual wakeup, so delayed tasks cannot form a burst.
        state.next = Instant::now() + SPACING;
        Ok(())
    }
    pub(super) async fn defer(&self, delay: Duration) {
        let mut state = self.0.lock().await;
        if let Some(next) = Instant::now().checked_add(delay) {
            state.next = state.next.max(next);
        } else {
            state.blocked = true;
        }
    }
}

pub(super) fn retry_delay(headers: &reqwest::header::HeaderMap, retry: usize) -> Duration {
    let fallback = Duration::from_secs(1 << retry.min(2));
    let Some(value) = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
    else {
        return fallback;
    };
    let seconds = value.parse::<u64>().ok().or_else(|| {
        jiff::fmt::rfc2822::parse(value).ok().map(|date| {
            date.timestamp()
                .duration_since(jiff::Timestamp::now())
                .as_secs()
                .max(1) as u64
        })
    });
    seconds
        .map(|value| Duration::from_secs(value.max(1)))
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delay(value: &str, retry: usize) -> Duration {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, value.parse().unwrap());
        retry_delay(&headers, retry)
    }

    #[test]
    fn retry_after_takes_seconds_or_an_http_date_and_otherwise_backs_off() {
        let none = reqwest::header::HeaderMap::new();
        for (retry, seconds) in [(0, 1), (1, 2), (2, 4), (3, 4), (usize::MAX, 4)] {
            assert_eq!(retry_delay(&none, retry), Duration::from_secs(seconds));
        }
        assert_eq!(delay("120", 0), Duration::from_secs(120));
        assert_eq!(delay("0", 2), Duration::from_secs(1));
        // A date that has passed is read, so the wait is the one second minimum
        // rather than the four second backoff.
        for value in [
            "Wed, 21 Oct 2015 07:28:00 GMT",
            "Wed, 21 Oct 2015 07:28:00 +0000",
            "Wed, 21 Oct 2015 07:28:00 -0400",
            "21 Oct 2015 07:28:00 GMT",
            "Wed, 21 Oct 2015 07:28 GMT",
            "wed, 21 oct 2015 07:28:00 gmt",
            "Wed, 21 Oct 2015 07:28:00 EST",
            "Wed, 21 Oct 15 07:28:00 GMT",
        ] {
            assert_eq!(delay(value, 2), Duration::from_secs(1), "{value:?}");
        }
        for value in [
            "",
            "-5",
            "1.5",
            "soon",
            "2015-10-21T07:28:00Z",
            "Thu, 21 Oct 2015 07:28:00 GMT",
            "Wednesday, 21-Oct-15 07:28:00 GMT",
            "Wed Oct 21 07:28:00 2015",
            "Wed, 21 Oct 2015 07:28:00",
            "Wed, 32 Oct 2015 07:28:00 GMT",
        ] {
            assert_eq!(delay(value, 2), Duration::from_secs(4), "{value:?}");
        }
        let future = delay("31 Dec 9998 23:59:59 GMT", 0);
        assert!(future > Duration::from_secs(250_000_000_000));
    }

    #[test]
    fn a_retry_after_date_with_an_unknown_zone_name_is_read_as_utc() {
        // chrono refused a zone name it did not know and backed off instead.
        // jiff follows RFC 5322 section 4.3 and reads such a name as UTC.
        for value in [
            "Wed, 21 Oct 2015 07:28:00 UTC",
            "Wed, 21 Oct 2015 07:28:00 XYZ",
        ] {
            assert_eq!(delay(value, 2), Duration::from_secs(1), "{value:?}");
        }
    }
}
