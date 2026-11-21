//! Today and month selection use a caller-captured instant in the Jira account zone.
use super::{DateRange, Error};
use crate::account_zone::{AccountZone, CalendarError};
use jiff::{Timestamp, civil::Date};

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
    pub fn today_at(&self, instant: Timestamp) -> Result<Date, Error> {
        Ok(self.zone.at(instant).date())
    }
    pub fn current_month_at(&self, instant: Timestamp) -> Result<DateRange, Error> {
        DateRange::month_containing(self.today_at(instant)?)
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
