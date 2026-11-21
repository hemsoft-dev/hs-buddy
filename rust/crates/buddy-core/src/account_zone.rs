//! One captured IANA zone, resolved against the same bundled data on every OS.
use jiff::{
    Timestamp, Zoned,
    tz::{TimeZone, TimeZoneDatabase},
};

pub(crate) enum CalendarError {
    TimezoneUnavailable,
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
    pub(crate) fn at(&self, instant: Timestamp) -> Zoned {
        instant.to_zoned(self.zone.clone())
    }
}
