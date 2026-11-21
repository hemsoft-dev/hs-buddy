//! A captured account zone drives all grouping and overdue checks.
use crate::account_zone::{AccountZone, CalendarError};
use jiff::{Timestamp, Zoned, civil::Date};

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
    fn local_at(&self, instant: Timestamp) -> Zoned {
        self.zone.at(instant)
    }
    pub fn today_at(&self, now: Timestamp) -> Result<Date, Error> {
        Ok(self.local_at(now).date())
    }
    pub fn due_day(&self, due: &DueDate) -> Result<Date, Error> {
        match &due.kind {
            DueKind::Calendar(day) => Ok(*day),
            DueKind::Floating(time) => Ok(time.date()),
            DueKind::Instant(time) => Ok(self.local_at(time.timestamp()).date()),
        }
    }
    pub fn due_is_overdue_at(&self, due: &DueDate, now: Timestamp) -> Result<bool, Error> {
        Ok(due.overdue_at(&self.local_at(now)))
    }
    pub fn schedule(
        &self,
        tasks: &[Task],
        now: Timestamp,
        days: u8,
    ) -> Result<Vec<DayGroup>, Error> {
        super::schedule::schedule_by_day(tasks, self.today_at(now)?, days, |due| self.due_day(due))
    }
}

impl From<CalendarError> for Error {
    fn from(error: CalendarError) -> Self {
        match error {
            CalendarError::TimezoneUnavailable => Self::TimezoneUnavailable,
        }
    }
}

#[cfg(test)]
#[path = "calendar_tests.rs"]
mod tests;
