//! One captured IANA zone, resolved against the same bundled data on every OS.
use chrono::{DateTime, FixedOffset, Utc};
use jiff::{
    Timestamp,
    tz::{TimeZone, TimeZoneDatabase},
};

pub(crate) enum CalendarError {
    TimezoneUnavailable,
    InvalidDate,
}
pub(crate) struct AccountZone {
    zone: TimeZone,
    name: String,
}
impl AccountZone {
    pub(crate) fn from_name(name: Option<&str>) -> Result<Self, CalendarError> {
        let name = name.ok_or(CalendarError::TimezoneUnavailable)?;
        let zone = TimeZoneDatabase::bundled()
            .get(name)
            .map_err(|_| CalendarError::TimezoneUnavailable)?;
        if zone.is_unknown() {
            return Err(CalendarError::TimezoneUnavailable);
        }
        let name = zone.iana_name().unwrap_or(name).to_owned();
        Ok(Self { zone, name })
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn at(
        &self,
        instant: DateTime<Utc>,
    ) -> Result<DateTime<FixedOffset>, CalendarError> {
        let timestamp =
            Timestamp::new(instant.timestamp(), instant.timestamp_subsec_nanos() as i32)
                .map_err(|_| CalendarError::InvalidDate)?;
        let offset = FixedOffset::east_opt(self.zone.to_offset(timestamp).seconds())
            .ok_or(CalendarError::InvalidDate)?;
        Ok(instant.with_timezone(&offset))
    }
}
