//! The REST quota shown by the overview's gauge, and GitHub's retry windows
//! for it and for the workbench's searches, kept per GitHub login.
//!
//! GitHub meters requests per credential, and the credential is chosen by
//! login alone, so one login configured in several namespaces shares one
//! entry. The tracker lives as long as the GitHub view, not the overview: a
//! reply that arrives after the overview is left is still recorded, and a
//! retry window GitHub asked for is honoured when the overview is reopened.
//!
//! Each rate-limited resource has its own [`RetryGate`]: GitHub limits
//! `/search` separately from the core REST quota, so a throttled search
//! holds back only the searches.
//!
//! The tracker is the only store of the reading: `/rate_limit` is never
//! served from or stored in the per-namespace response cache, a reading
//! reported before the one held is never applied, and a reading under a
//! minute old is reused instead of read again.
use buddy_core::github::{AccountScope, RateLimit};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Electron's gauge re-reads the quota every 60 seconds, and a throttled read
/// never waits less than that.
pub(super) const RATE_LIMIT_REFRESH: Duration = Duration::from_secs(60);

/// A resource GitHub rate-limits on its own, with its own retry window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Resource {
    /// `/rate_limit`, read for the gauge.
    Quota,
    /// `/search/issues`, read for the workbench.
    Search,
}

/// GitHub's retry window for one login and resource.
#[derive(Debug, Default)]
struct RetryGate {
    /// No request is sent before this. It only ever moves later, except that
    /// a success clears a deadline that was set before its request was issued.
    not_before: Option<Instant>,
    /// The newest request id issued when `not_before` was last moved.
    epoch: u64,
    /// When GitHub last throttled this resource.
    held_at: Option<Instant>,
}

/// GitHub throttled a login's searches: how long ago, and how long the
/// login must still wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SearchHold {
    pub(super) since: Duration,
    pub(super) wait: Duration,
}

impl SearchHold {
    /// The wait in whole seconds, rounded up, as GitHub's `Retry-After`.
    pub(super) fn wait_seconds(&self) -> u64 {
        (self.wait.as_secs() + u64::from(self.wait.subsec_nanos() > 0)).max(1)
    }
}

impl RetryGate {
    fn open(&self, now: Instant) -> bool {
        self.not_before.is_none_or(|at| now >= at)
    }

    /// GitHub asked to wait `seconds`; never less than a minute, and never
    /// earlier than a window already set. `issued` is the newest request id.
    fn hold(&mut self, now: Instant, seconds: u64, issued: u64) {
        let deadline = now + Duration::from_secs(RATE_LIMIT_REFRESH.as_secs().max(seconds));
        self.not_before = Some(self.not_before.map_or(deadline, |at| at.max(deadline)));
        self.epoch = issued;
        self.held_at = Some(now);
    }

    /// Request `id` succeeded: a window set before it was issued is over.
    fn release(&mut self, id: u64) {
        if self.epoch < id {
            self.not_before = None;
        }
    }
}

/// One issued workbench search. Its reply is recorded with
/// [`QuotaTracker::finish_search`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SearchRequest {
    login: String,
    id: u64,
}

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
}

#[derive(Debug, Default)]
pub(super) struct QuotaTracker {
    logins: HashMap<String, LoginQuota>,
    gates: HashMap<(String, Resource), RetryGate>,
    /// Request ids are unique across logins and resources, and only grow.
    issued: u64,
}

fn login(account: &AccountScope) -> String {
    account.username().to_ascii_lowercase()
}

impl QuotaTracker {
    fn get(&self, account: &AccountScope) -> Option<&LoginQuota> {
        self.logins.get(&login(account))
    }

    fn gate(&self, account: &AccountScope, resource: Resource) -> Option<&RetryGate> {
        self.gates.get(&(login(account), resource))
    }

    fn gate_mut(&mut self, login: &str, resource: Resource) -> &mut RetryGate {
        self.gates.entry((login.to_string(), resource)).or_default()
    }

    /// Whether GitHub's quota retry window, if any, has passed for this login.
    pub(super) fn may_send(&self, account: &AccountScope, now: Instant) -> bool {
        self.gate(account, Resource::Quota)
            .is_none_or(|gate| gate.open(now))
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
        let gate = self
            .gates
            .entry((request.login.clone(), Resource::Quota))
            .or_default();
        let quota = self.logins.entry(request.login.clone()).or_default();
        if let QuotaReply::Throttled {
            retry_after_seconds,
        } = reply
        {
            let mut seconds = retry_after_seconds;
            if let Some(last) = quota.reading.filter(|last| last.remaining == 0) {
                seconds = seconds.max(last.reset.saturating_sub(now_seconds).max(0) as u64);
            }
            gate.hold(now, seconds, issued);
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
                gate.release(request.id);
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

    /// Issue a workbench search if GitHub's search window for the login has
    /// passed; while it is open, no search is sent.
    pub(super) fn begin_search(
        &mut self,
        account: &AccountScope,
        now: Instant,
    ) -> Option<SearchRequest> {
        if self.search_hold(account, now).is_some() {
            return None;
        }
        self.issued += 1;
        Some(SearchRequest {
            login: login(account),
            id: self.issued,
        })
    }

    /// Record a workbench search's reply: `throttled` is the longest
    /// `Retry-After` GitHub gave either search, if one was limited. A
    /// throttle always holds the login's searches back, even for a request
    /// that was superseded; a success ends only a window set before it.
    pub(super) fn finish_search(
        &mut self,
        request: &SearchRequest,
        throttled: Option<u64>,
        now: Instant,
    ) {
        let issued = self.issued;
        let gate = self.gate_mut(&request.login, Resource::Search);
        match throttled {
            Some(seconds) => gate.hold(now, seconds, issued),
            None => gate.release(request.id),
        }
    }

    /// GitHub's search window for the login, while it is still open.
    pub(super) fn search_hold(&self, account: &AccountScope, now: Instant) -> Option<SearchHold> {
        let gate = self.gate(account, Resource::Search)?;
        let at = gate.not_before.filter(|at| now < *at)?;
        Some(SearchHold {
            since: gate
                .held_at
                .map_or(Duration::ZERO, |held| now.saturating_duration_since(held)),
            wait: at - now,
        })
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
        self.gate(account, Resource::Quota)
            .and_then(|gate| gate.not_before)
    }

    #[cfg(test)]
    pub(super) fn search_not_before(&self, account: &AccountScope) -> Option<Instant> {
        self.gate(account, Resource::Search)
            .and_then(|gate| gate.not_before)
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
        }
        if expire_deadlines {
            self.expire_windows_for_test();
        }
    }

    /// End every retry window, as if GitHub's deadlines had passed.
    #[cfg(test)]
    pub(super) fn expire_windows_for_test(&mut self) {
        let now = Instant::now();
        for gate in self.gates.values_mut() {
            gate.not_before = gate.not_before.map(|at| at.min(now));
        }
    }
}

#[cfg(test)]
mod tests;
