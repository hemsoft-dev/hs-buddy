//! A captured account zone drives all grouping and overdue checks.
use crate::account_zone::{AccountZone, CalendarError};
use chrono::{DateTime, FixedOffset, NaiveDate, Utc};

use super::{DayGroup, DueDate, DueKind, Error, Profile, Task};

pub struct AccountCalendar {
    zone: AccountZone,
}
impl AccountCalendar {
    pub fn from_profile(profile: &Profile) -> Result<Self, Error> {
        Ok(Self {
            zone: AccountZone::from_name(profile.timezone.as_deref())?,
        })
    }
    pub fn zone_name(&self) -> &str {
        self.zone.name()
    }
    fn local_at(&self, instant: DateTime<Utc>) -> Result<DateTime<FixedOffset>, Error> {
        self.zone.at(instant).map_err(Into::into)
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

impl From<CalendarError> for Error {
    fn from(error: CalendarError) -> Self {
        match error {
            CalendarError::TimezoneUnavailable => Self::TimezoneUnavailable,
            CalendarError::InvalidDate => Self::InvalidDate,
        }
    }
}

#[cfg(test)]
#[path = "calendar_tests.rs"]
mod tests;
