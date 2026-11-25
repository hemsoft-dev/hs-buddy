//! The REST quota shown by the overview's gauge, kept per GitHub login.
//!
//! GitHub meters `/rate_limit` per credential, and the credential is chosen by
//! login alone, so one login configured in several namespaces shares one
//! entry. The tracker lives as long as the GitHub view, not the overview: a
//! reply that arrives after the overview is left is still recorded, and a
//! retry window GitHub asked for is honoured when the overview is reopened.
//!
//! The tracker is the only store of the reading: `/rate_limit` is never
//! served from the per-namespace response cache, a reading reported before
//! the one held is never applied, and a reading under a minute old is reused
//! instead of read again.
use buddy_core::github::{AccountScope, RateLimit};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Electron's gauge re-reads the quota every 60 seconds, and a throttled read
/// never waits less than that.
pub(super) const RATE_LIMIT_REFRESH: Duration = Duration::from_secs(60);

/// One issued quota read. Its reply is recorded with [`QuotaTracker::finish`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct QuotaRequest {
    login: String,
    id: u64,
}

/// What GitHub answered a quota read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QuotaReply {
    /// A reading, and how long ago GitHub reported it.
    Read(RateLimit, Duration),
    /// GitHub limited the request and asked for this `Retry-After`.
    Throttled { retry_after_seconds: u64 },
    /// Any other failure, including a cancelled read.
    Failed,
}

#[derive(Debug, Default)]
struct LoginQuota {
    /// The newest successful reading. A failure hides it from the gauge but
    /// keeps it, so a later throttle still knows an exhausted quota's reset.
    reading: Option<RateLimit>,
    /// When GitHub reported `reading`. A reading reported earlier than this
    /// is older than the one held, and is never applied.
    reported_at: Option<Instant>,
    /// The newest reply failed, so the gauge shows nothing.
    failed: bool,
    /// When GitHub reported the shown reading, or when the newest read failed.
    read_at: Option<Instant>,
    /// The newest request issued for this login, and whether it is unanswered.
    latest: u64,
    pending: bool,
    /// No request is sent before this. It only ever moves later, except that
    /// a success clears a deadline that was set before its request was issued.
    not_before: Option<Instant>,
    /// The newest request id issued when `not_before` was last moved.
    deadline_epoch: u64,
}

#[derive(Debug, Default)]
pub(super) struct QuotaTracker {
    logins: HashMap<String, LoginQuota>,
    /// Request ids are unique across logins and only grow.
    issued: u64,
}

fn login(account: &AccountScope) -> String {
    account.username().to_ascii_lowercase()
}

impl QuotaTracker {
    fn get(&self, account: &AccountScope) -> Option<&LoginQuota> {
        self.logins.get(&login(account))
    }

    /// Whether GitHub's retry window, if any, has passed for this login.
    pub(super) fn may_send(&self, account: &AccountScope, now: Instant) -> bool {
        self.get(account)
            .and_then(|quota| quota.not_before)
            .is_none_or(|at| now >= at)
    }

    /// Whether the minute poll should read again: it may send, nothing is
    /// in flight, and the shown reading or failure is a minute old.
    pub(super) fn due(&self, account: &AccountScope, now: Instant) -> bool {
        self.may_send(account, now)
            && self.get(account).is_none_or(|quota| {
                !quota.pending
                    && quota
                        .read_at
                        .is_none_or(|at| now.saturating_duration_since(at) >= RATE_LIMIT_REFRESH)
            })
    }

    /// Issue a request if the login may send now. The returned request
    /// supersedes every earlier one for that login.
    pub(super) fn begin(&mut self, account: &AccountScope, now: Instant) -> Option<QuotaRequest> {
        if !self.may_send(account, now) {
            return None;
        }
        self.issued += 1;
        let login = login(account);
        let quota = self.logins.entry(login.clone()).or_default();
        quota.latest = self.issued;
        quota.pending = true;
        Some(QuotaRequest {
            login,
            id: self.issued,
        })
    }

    /// Record a reply. A throttle always holds the login back, even when the
    /// request was superseded; anything else is applied only for the login's
    /// newest request, and a reading only if GitHub reported it no earlier
    /// than the one held. Returns whether the shown quota changed.
    pub(super) fn finish(
        &mut self,
        request: &QuotaRequest,
        reply: QuotaReply,
        now: Instant,
        now_seconds: i64,
    ) -> bool {
        let issued = self.issued;
        let quota = self.logins.entry(request.login.clone()).or_default();
        if let QuotaReply::Throttled {
            retry_after_seconds,
        } = reply
        {
            let mut seconds = RATE_LIMIT_REFRESH.as_secs().max(retry_after_seconds);
            if let Some(last) = quota.reading.filter(|last| last.remaining == 0) {
                seconds = seconds.max(last.reset.saturating_sub(now_seconds).max(0) as u64);
            }
            let deadline = now + Duration::from_secs(seconds);
            quota.not_before = Some(quota.not_before.map_or(deadline, |at| at.max(deadline)));
            quota.deadline_epoch = issued;
        }
        if request.id != quota.latest {
            return false;
        }
        quota.pending = false;
        match reply {
            QuotaReply::Read(limit, age) => {
                let reported = now.checked_sub(age).unwrap_or(now);
                if quota.reported_at.is_some_and(|held| reported < held) {
                    // An older reading says nothing about the quota now, nor
                    // about a retry window, so the held one stays.
                    return false;
                }
                if quota.deadline_epoch < request.id {
                    quota.not_before = None;
                }
                quota.reading = Some(limit);
                quota.reported_at = Some(reported);
                quota.failed = false;
                quota.read_at = Some(reported);
            }
            QuotaReply::Throttled { .. } | QuotaReply::Failed => {
                quota.failed = true;
                quota.read_at = Some(now);
            }
        }
        true
    }

    /// The reading the gauge shows; none before the first reading or after
    /// a failed one.
    pub(super) fn shown(&self, account: &AccountScope) -> Option<&RateLimit> {
        self.get(account)
            .filter(|quota| !quota.failed)
            .and_then(|quota| quota.reading.as_ref())
    }

    /// The newest read for this login is still unanswered.
    #[cfg(test)]
    pub(super) fn pending(&self, account: &AccountScope) -> bool {
        self.get(account).is_some_and(|quota| quota.pending)
    }

    /// The newest reply for this login failed.
    #[cfg(test)]
    pub(super) fn failed(&self, account: &AccountScope) -> bool {
        self.get(account).is_some_and(|quota| quota.failed)
    }

    #[cfg(test)]
    pub(super) fn read_at(&self, account: &AccountScope) -> Option<Instant> {
        self.get(account).and_then(|quota| quota.read_at)
    }

    #[cfg(test)]
    pub(super) fn not_before(&self, account: &AccountScope) -> Option<Instant> {
        self.get(account).and_then(|quota| quota.not_before)
    }

    /// Age every reading by a minute and end every retry window, as if time
    /// had passed.
    #[cfg(test)]
    pub(super) fn age_for_test(&mut self, expire_deadlines: bool) {
        for quota in self.logins.values_mut() {
            quota.read_at = quota
                .read_at
                .and_then(|at| at.checked_sub(RATE_LIMIT_REFRESH));
            quota.reported_at = quota
                .reported_at
                .and_then(|at| at.checked_sub(RATE_LIMIT_REFRESH));
            if expire_deadlines {
                quota.not_before = quota.not_before.map(|_| Instant::now());
            }
        }
    }
}

#[cfg(test)]
mod tests;
