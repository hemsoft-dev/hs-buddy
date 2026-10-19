//! Today and month selection use a caller-captured instant in the Jira account zone.
use super::{DateRange, Error};
use crate::account_zone::{AccountZone, CalendarError};
use chrono::{DateTime, NaiveDate, Utc};

pub struct AccountCalendar {
    zone: AccountZone,
}
impl AccountCalendar {
    pub fn from_zone(name: Option<&str>) -> Result<Self, Error> {
        Ok(Self {
            zone: AccountZone::from_name(name)?,
        })
    }
    pub fn zone_name(&self) -> &str {
        self.zone.name()
    }
    pub fn today_at(&self, instant: DateTime<Utc>) -> Result<NaiveDate, Error> {
        Ok(self.zone.at(instant)?.date_naive())
    }
    pub fn current_month_at(&self, instant: DateTime<Utc>) -> Result<DateRange, Error> {
        DateRange::month_containing(self.today_at(instant)?)
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
