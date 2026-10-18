//! A captured account zone drives all grouping and overdue checks.
use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use jiff::{
    Timestamp,
    tz::{TimeZone, TimeZoneDatabase},
};

use super::{DayGroup, DueDate, DueKind, Error, Profile, Task};

pub struct AccountCalendar {
    zone: TimeZone,
    name: String,
}
impl AccountCalendar {
    pub fn from_profile(profile: &Profile) -> Result<Self, Error> {
        let name = profile
            .timezone
            .as_deref()
            .ok_or(Error::TimezoneUnavailable)?;
        // Use the same reviewed bundled data on every OS, regardless of TZDIR
        // or a stale/missing host database. No machine-zone fallback.
        let zone = TimeZoneDatabase::bundled()
            .get(name)
            .map_err(|_| Error::TimezoneUnavailable)?;
        if zone.is_unknown() {
            return Err(Error::TimezoneUnavailable);
        }
        let name = zone.iana_name().unwrap_or(name).to_owned();
        Ok(Self { zone, name })
    }
    pub fn zone_name(&self) -> &str {
        &self.name
    }
    fn local_at(&self, instant: DateTime<Utc>) -> Result<DateTime<FixedOffset>, Error> {
        let timestamp =
            Timestamp::new(instant.timestamp(), instant.timestamp_subsec_nanos() as i32)
                .map_err(|_| Error::InvalidDate)?;
        let offset = FixedOffset::east_opt(self.zone.to_offset(timestamp).seconds())
            .ok_or(Error::InvalidDate)?;
        Ok(instant.with_timezone(&offset))
    }
    pub fn today_at(&self, now: DateTime<Utc>) -> Result<NaiveDate, Error> {
        Ok(self.local_at(now)?.date_naive())
    }
    pub fn due_day(&self, due: &DueDate) -> Result<NaiveDate, Error> {
        match &due.kind {
            DueKind::Calendar(day) => Ok(*day),
            DueKind::Floating(time) => Ok(time.date()),
            DueKind::Instant(time) => Ok(self.local_at(time.with_timezone(&Utc))?.date_naive()),
        }
    }
    pub fn due_is_overdue_at(&self, due: &DueDate, now: DateTime<Utc>) -> Result<bool, Error> {
        Ok(due.overdue_at(&self.local_at(now)?))
    }
    pub fn schedule(
        &self,
        tasks: &[Task],
        now: DateTime<Utc>,
        days: u8,
    ) -> Result<Vec<DayGroup>, Error> {
        super::schedule::schedule_by_day(tasks, self.today_at(now)?, days, |due| self.due_day(due))
    }
}

#[cfg(test)]
#[path = "calendar_tests.rs"]
mod tests;
