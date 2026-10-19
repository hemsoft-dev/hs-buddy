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
        chrono::DateTime::parse_from_rfc2822(value)
            .ok()
            .map(|date| {
                (date.with_timezone(&chrono::Utc) - chrono::Utc::now())
                    .num_seconds()
                    .max(1) as u64
            })
    });
    seconds
        .map(|value| Duration::from_secs(value.max(1)))
        .unwrap_or(fallback)
}
