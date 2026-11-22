//! The date headings inside each workspace group of the session explorer,
//! matching `getDateGroup` in Electron's `SessionExplorer.tsx`.
//!
//! Electron starts from local midnight today, then steps back a fixed 24 hours
//! for Yesterday and a fixed 7 x 24 hours for This Week. Those two boundaries
//! are elapsed time, not calendar days, so across a daylight-saving change they
//! land an hour off local midnight, exactly as Electron's do.
use jiff::Zoned;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DateBucket {
    Today,
    Yesterday,
    ThisWeek,
    Older,
}

impl DateBucket {
    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Yesterday => "Yesterday",
            Self::ThisWeek => "This Week",
            Self::Older => "Older",
        }
    }
}

/// The local midnight that starts `now`'s day, in epoch milliseconds.
fn start_of_today(now: &Zoned) -> i64 {
    now.start_of_day()
        .map(|start| start.timestamp().as_millisecond())
        .unwrap_or_else(|_| now.timestamp().as_millisecond())
}

/// Buckets one session by its modification time. A future time counts as
/// Today, and an unknown time (0) is Older, as in Electron.
pub fn date_bucket(modified_ms: u64, now: &Zoned) -> DateBucket {
    let modified = i64::try_from(modified_ms).unwrap_or(i64::MAX);
    let today = start_of_today(now);
    if modified >= today {
        DateBucket::Today
    } else if modified >= today.saturating_sub(DAY_MS) {
        DateBucket::Yesterday
    } else if modified >= today.saturating_sub(7 * DAY_MS) {
        DateBucket::ThisWeek
    } else {
        DateBucket::Older
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;
    use jiff::tz::TimeZone;

    fn at(zone: &str, y: i16, m: i8, d: i8, h: i8, min: i8) -> Zoned {
        date(y, m, d)
            .at(h, min, 0, 0)
            .to_zoned(TimeZone::get(zone).unwrap())
            .unwrap()
    }

    fn ms(zoned: &Zoned) -> u64 {
        zoned.timestamp().as_millisecond() as u64
    }

    #[test]
    fn boundaries_are_inclusive_at_local_midnight_and_whole_days_before_it() {
        let zone = "Europe/Berlin";
        let now = at(zone, 2026, 6, 17, 15, 30);
        let midnight = ms(&at(zone, 2026, 6, 17, 0, 0));
        let day = DAY_MS as u64;
        for (modified, bucket) in [
            (ms(&now) + 60_000, DateBucket::Today),
            (ms(&now), DateBucket::Today),
            (midnight, DateBucket::Today),
            (midnight - 1, DateBucket::Yesterday),
            (midnight - day, DateBucket::Yesterday),
            (midnight - day - 1, DateBucket::ThisWeek),
            (midnight - 7 * day, DateBucket::ThisWeek),
            (midnight - 7 * day - 1, DateBucket::Older),
            (0, DateBucket::Older),
        ] {
            assert_eq!(date_bucket(modified, &now), bucket, "{modified}");
        }
    }

    #[test]
    fn today_follows_the_viewers_zone_not_utc() {
        // 23:30 UTC on the 16th is already the 17th in Tokyo and still the 16th in New York.
        let modified = ms(&at("UTC", 2026, 6, 16, 23, 30));
        assert_eq!(
            date_bucket(modified, &at("Asia/Tokyo", 2026, 6, 17, 12, 0)),
            DateBucket::Today
        );
        assert_eq!(
            date_bucket(modified, &at("America/New_York", 2026, 6, 17, 12, 0)),
            DateBucket::Yesterday
        );
    }

    #[test]
    fn yesterday_is_a_fixed_24_hours_across_daylight_saving_changes() {
        let zone = "America/New_York";
        // Clocks sprang forward on 2026-03-08, so that day had 23 hours. Electron's
        // Yesterday reaches back 24 hours to 23:00 on the 7th.
        let now = at(zone, 2026, 3, 9, 9, 0);
        assert_eq!(
            date_bucket(ms(&at(zone, 2026, 3, 7, 23, 30)), &now),
            DateBucket::Yesterday
        );
        assert_eq!(
            date_bucket(ms(&at(zone, 2026, 3, 7, 22, 30)), &now),
            DateBucket::ThisWeek
        );
        // Clocks fell back on 2026-11-01, a 25-hour day. Its first local hour is
        // more than 24 hours before midnight on the 2nd, so it is This Week.
        let now = at(zone, 2026, 11, 2, 9, 0);
        assert_eq!(
            date_bucket(ms(&at(zone, 2026, 11, 1, 0, 30)), &now),
            DateBucket::ThisWeek
        );
        assert_eq!(
            date_bucket(ms(&at(zone, 2026, 11, 1, 1, 30)), &now),
            DateBucket::Yesterday
        );
    }

    #[test]
    fn labels_and_order_match_electrons_headings() {
        let mut buckets = [
            DateBucket::Older,
            DateBucket::Today,
            DateBucket::ThisWeek,
            DateBucket::Yesterday,
        ];
        buckets.sort();
        assert_eq!(
            buckets.map(DateBucket::label),
            ["Today", "Yesterday", "This Week", "Older"]
        );
    }
}
