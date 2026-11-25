use super::*;

const NOW_SECONDS: i64 = 1_000;

fn account(username: &str, namespace: &str) -> AccountScope {
    AccountScope::new(username, namespace).unwrap()
}

fn reading(remaining: u64, reset: i64) -> RateLimit {
    RateLimit {
        limit: 5000,
        remaining,
        used: 5000 - remaining,
        reset,
    }
}

fn read(remaining: u64) -> QuotaReply {
    QuotaReply::Read(reading(remaining, NOW_SECONDS + 600), Duration::ZERO)
}

fn throttled(retry_after_seconds: u64) -> QuotaReply {
    QuotaReply::Throttled {
        retry_after_seconds,
    }
}

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

#[test]
fn an_older_success_never_clears_a_newer_requests_retry_window() {
    let first = account("First", "team");
    let mut quota = QuotaTracker::default();
    let now = Instant::now();
    let older = quota.begin(&first, now).unwrap();
    let newer = quota.begin(&first, now).unwrap();
    assert!(quota.finish(&newer, throttled(300), now, NOW_SECONDS));
    let deadline = quota.not_before(&first).unwrap();
    assert_eq!(deadline, now + secs(300));

    // The superseded request succeeds afterwards: neither its reading nor
    // the end of the window it implies is applied.
    assert!(!quota.finish(&older, read(4321), now, NOW_SECONDS));
    assert_eq!(quota.not_before(&first), Some(deadline));
    assert!(quota.shown(&first).is_none() && quota.failed(&first));
    assert!(!quota.may_send(&first, now + secs(299)));
    assert!(quota.begin(&first, now + secs(299)).is_none());
    assert!(quota.may_send(&first, now + secs(300)));
}

#[test]
fn a_superseded_throttle_still_holds_the_login_back() {
    let first = account("First", "team");
    let mut quota = QuotaTracker::default();
    let now = Instant::now();
    let older = quota.begin(&first, now).unwrap();
    let newer = quota.begin(&first, now).unwrap();
    assert!(quota.finish(&newer, read(4321), now, NOW_SECONDS));
    // GitHub's window applies to the credential, whichever request saw it.
    assert!(!quota.finish(&older, throttled(120), now, NOW_SECONDS));
    assert_eq!(quota.not_before(&first), Some(now + secs(120)));
    // The newest reading is still shown.
    assert_eq!(quota.shown(&first).map(|limit| limit.remaining), Some(4321));
}

#[test]
fn a_retry_window_only_moves_later() {
    let first = account("First", "team");
    let mut quota = QuotaTracker::default();
    let now = Instant::now();
    let long = quota.begin(&first, now).unwrap();
    let short = quota.begin(&first, now).unwrap();
    quota.finish(&long, throttled(600), now, NOW_SECONDS);
    quota.finish(&short, throttled(5), now, NOW_SECONDS);
    assert_eq!(quota.not_before(&first), Some(now + secs(600)));
}

#[test]
fn a_success_clears_only_a_window_set_before_its_request() {
    let first = account("First", "team");
    let mut quota = QuotaTracker::default();
    let now = Instant::now();
    let throttled_read = quota.begin(&first, now).unwrap();
    quota.finish(&throttled_read, throttled(60), now, NOW_SECONDS);
    let later = now + secs(60);
    let request = quota.begin(&first, later).unwrap();
    assert!(quota.finish(&request, read(4000), later, NOW_SECONDS));
    assert_eq!(quota.not_before(&first), None);
    assert_eq!(quota.shown(&first).map(|limit| limit.remaining), Some(4000));
    assert!(!quota.pending(&first));
}

#[test]
fn a_throttle_waits_at_least_a_minute_and_until_an_exhausted_reset() {
    let now = Instant::now();
    let window = |last: Option<RateLimit>, retry_after: u64, now_seconds: i64| {
        let first = account("First", "team");
        let mut quota = QuotaTracker::default();
        if let Some(last) = last {
            let request = quota.begin(&first, now).unwrap();
            quota.finish(
                &request,
                QuotaReply::Read(last, Duration::ZERO),
                now,
                now_seconds,
            );
        }
        let request = quota.begin(&first, now).unwrap();
        quota.finish(&request, throttled(retry_after), now, now_seconds);
        quota.not_before(&first).unwrap() - now
    };
    assert_eq!(window(None, 5, 1_000), secs(60));
    assert_eq!(window(None, 300, 1_000), secs(300));
    // A quota last seen exhausted is not read again before it resets.
    assert_eq!(window(Some(reading(0, 1_900)), 30, 1_000), secs(900));
    assert_eq!(window(Some(reading(10, 1_900)), 30, 1_000), secs(60));
    assert_eq!(window(Some(reading(0, 1_900)), 30, 5_000), secs(60));
}

#[test]
fn a_late_throttle_still_knows_the_reset_of_an_exhausted_quota() {
    let first = account("First", "team");
    let mut quota = QuotaTracker::default();
    let now = Instant::now();
    let request = quota.begin(&first, now).unwrap();
    quota.finish(
        &request,
        QuotaReply::Read(reading(0, NOW_SECONDS + 900), Duration::ZERO),
        now,
        NOW_SECONDS,
    );
    // An ordinary failure hides the gauge but keeps the reading's reset.
    let failed = quota.begin(&first, now).unwrap();
    quota.finish(&failed, QuotaReply::Failed, now, NOW_SECONDS);
    assert!(quota.shown(&first).is_none());
    assert_eq!(quota.not_before(&first), None);
    let request = quota.begin(&first, now).unwrap();
    quota.finish(&request, throttled(5), now, NOW_SECONDS);
    assert_eq!(quota.not_before(&first), Some(now + secs(900)));
}

#[test]
fn one_login_in_two_namespaces_shares_its_quota_and_retry_window() {
    let team = account("First", "team");
    let personal = account("first", "first");
    let other_login = account("Second", "team");
    let mut quota = QuotaTracker::default();
    let now = Instant::now();
    let request = quota.begin(&team, now).unwrap();
    quota.finish(&request, throttled(300), now, NOW_SECONDS);
    assert!(!quota.may_send(&personal, now));
    assert!(quota.begin(&personal, now).is_none());
    assert!(!quota.due(&personal, now + RATE_LIMIT_REFRESH));
    // Another login has its own credential and its own window.
    assert!(quota.may_send(&other_login, now));
    let request = quota.begin(&other_login, now).unwrap();
    quota.finish(&request, read(4321), now, NOW_SECONDS);
    assert_eq!(
        quota.shown(&other_login).map(|limit| limit.remaining),
        Some(4321)
    );
    assert!(quota.shown(&team).is_none());
    assert_eq!(quota.not_before(&team), Some(now + secs(300)));

    // A reading for the login is shown in each of its namespaces.
    let later = now + secs(300);
    let request = quota.begin(&personal, later).unwrap();
    quota.finish(&request, read(1200), later, NOW_SECONDS);
    assert_eq!(quota.shown(&team).map(|limit| limit.remaining), Some(1200));
}

#[test]
fn the_poll_waits_for_the_minute_and_for_an_unanswered_read() {
    let first = account("First", "team");
    let mut quota = QuotaTracker::default();
    let now = Instant::now();
    assert!(quota.due(&first, now));
    let request = quota.begin(&first, now).unwrap();
    assert!(!quota.due(&first, now + RATE_LIMIT_REFRESH));
    quota.finish(&request, read(4321), now, NOW_SECONDS);
    assert!(!quota.due(&first, now + secs(59)));
    assert!(quota.due(&first, now + RATE_LIMIT_REFRESH));

    // A reading keeps the time GitHub reported it, not when it arrived.
    let mut quota = QuotaTracker::default();
    let request = quota.begin(&first, now).unwrap();
    quota.finish(
        &request,
        QuotaReply::Read(reading(4000, NOW_SECONDS + 600), secs(40)),
        now,
        NOW_SECONDS,
    );
    assert_eq!(quota.read_at(&first), Some(now - secs(40)));
    assert!(quota.due(&first, now + secs(20)));
}

#[test]
fn a_reading_older_than_the_held_one_is_never_applied() {
    // Namespace A reads a nonzero quota, then namespace B reads the same
    // login's quota exhausted half a minute later.
    let team = account("First", "team");
    let solo = account("first", "solo");
    let mut quota = QuotaTracker::default();
    let now = Instant::now();
    let a = quota.begin(&team, now).unwrap();
    assert!(quota.finish(&a, read(4321), now, NOW_SECONDS));
    let later = now + secs(30);
    let b = quota.begin(&solo, later).unwrap();
    let exhausted = reading(0, NOW_SECONDS + 900);
    assert!(quota.finish(
        &b,
        QuotaReply::Read(exhausted, Duration::ZERO),
        later,
        NOW_SECONDS
    ));
    // Reopening A gets a reading GitHub reported before B's, as a cached
    // reply would be. It is the newest request, but not the newest reading.
    let reopened = quota.begin(&team, later).unwrap();
    assert!(!quota.finish(
        &reopened,
        QuotaReply::Read(reading(4321, NOW_SECONDS + 600), secs(30)),
        later,
        NOW_SECONDS
    ));
    assert_eq!(quota.shown(&team), Some(&exhausted));
    assert_eq!(quota.read_at(&team), Some(later));
    assert!(!quota.pending(&team));

    // A short Retry-After after it still waits for the exhausted reset.
    let throttled_read = quota.begin(&team, later).unwrap();
    quota.finish(&throttled_read, throttled(5), later, NOW_SECONDS);
    assert_eq!(quota.not_before(&team), Some(later + secs(900)));
    assert!(quota.begin(&solo, later + secs(899)).is_none());
}

#[test]
fn a_reading_under_a_minute_old_is_reused_in_every_namespace_of_its_login() {
    let team = account("First", "team");
    let solo = account("first", "solo");
    let mut quota = QuotaTracker::default();
    let now = Instant::now();
    let request = quota.begin(&team, now).unwrap();
    quota.finish(&request, read(4321), now, NOW_SECONDS);
    assert!(!quota.due(&solo, now + secs(59)));
    assert_eq!(quota.shown(&solo).map(|limit| limit.remaining), Some(4321));
    assert!(quota.due(&solo, now + RATE_LIMIT_REFRESH));
}
