use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, TimeZone};
use serde::Deserialize;

use super::{Error, bounded_text};

#[derive(Clone)]
pub enum DueKind {
    Calendar(NaiveDate),
    Floating(NaiveDateTime),
    Instant(DateTime<FixedOffset>),
}

/// Keep the source representation for display and explicit edits. No Debug:
/// recurrence descriptions and task dates are private user data.
#[derive(Clone)]
pub struct DueDate {
    pub kind: DueKind,
    pub source_date: String,
    pub source_datetime: Option<String>,
    pub timezone: Option<String>,
    pub description: Option<String>,
    pub recurring: bool,
}

#[derive(Deserialize)]
pub(super) struct RawDue {
    date: String,
    datetime: Option<String>,
    timezone: Option<String>,
    string: Option<String>,
    #[serde(default)]
    is_recurring: bool,
}

fn valid_year(year: i32) -> bool {
    (1..=9999).contains(&year)
}

fn parse_kind(text: &str) -> Result<DueKind, Error> {
    if !bounded_text(text, 64, false) {
        return Err(Error::InvalidDate);
    }
    if text.len() == 10
        && text.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 4 | 7) {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
    {
        return NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .ok()
            .filter(|date| valid_year(date.year()))
            .map(DueKind::Calendar)
            .ok_or(Error::InvalidDate);
    }
    if let Ok(instant) = DateTime::parse_from_rfc3339(text) {
        return valid_year(instant.year())
            .then_some(DueKind::Instant(instant))
            .ok_or(Error::InvalidDate);
    }
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .filter(|date| valid_year(date.year()))
        .map(DueKind::Floating)
        .ok_or(Error::InvalidDate)
}

impl DueDate {
    pub(super) fn parse(raw: RawDue) -> Result<Self, Error> {
        let source_kind = parse_kind(&raw.date)?;
        let kind = match raw.datetime.as_deref() {
            Some(text) => match parse_kind(text)? {
                DueKind::Instant(instant) => {
                    if matches!(&source_kind, DueKind::Instant(source) if source != &instant) {
                        return Err(Error::InvalidDate);
                    }
                    DueKind::Instant(instant)
                }
                _ => return Err(Error::InvalidDate),
            },
            None => source_kind,
        };
        // The v1 guide represents fixed-zone times as UTC instants. A local
        // datetime with a named zone is unsupported, not a floating date.
        if matches!(&kind, DueKind::Floating(_)) && raw.timezone.is_some() {
            return Err(Error::InvalidDate);
        }
        if raw
            .timezone
            .as_ref()
            .is_some_and(|zone| zone.is_empty() || !bounded_text(zone, 128, false))
            || raw
                .string
                .as_ref()
                .is_some_and(|text| !bounded_text(text, 4096, true))
        {
            return Err(Error::InvalidDate);
        }
        Ok(Self {
            kind,
            source_date: raw.date,
            source_datetime: raw.datetime,
            timezone: raw.timezone,
            description: raw.string,
            recurring: raw.is_recurring,
        })
    }

    /// Calendar and floating due dates stay on their wall date. Only an
    /// explicit instant is converted to the account's supplied timezone.
    pub fn day_in<Zone: TimeZone>(&self, zone: &Zone) -> NaiveDate {
        match &self.kind {
            DueKind::Calendar(date) => *date,
            DueKind::Floating(date) => date.date(),
            DueKind::Instant(instant) => instant.with_timezone(zone).date_naive(),
        }
    }

    pub fn overdue_at<Zone: TimeZone>(&self, now: &DateTime<Zone>) -> bool {
        match &self.kind {
            DueKind::Calendar(date) => *date < now.date_naive(),
            DueKind::Floating(date) => *date < now.naive_local(),
            DueKind::Instant(instant) => instant.naive_utc() < now.naive_utc(),
        }
    }
}
