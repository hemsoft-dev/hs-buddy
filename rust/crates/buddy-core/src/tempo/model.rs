use std::collections::HashSet;

use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, Utc};
use serde::Deserialize;

use super::{Error, MAX_PAGE_ITEMS, MAX_RESPONSE_BYTES, bounded};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorklogId(u64);
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct IssueId(u64);
fn positive(value: u64) -> Result<u64, Error> {
    (value > 0 && value <= i64::MAX as u64)
        .then_some(value)
        .ok_or(Error::InvalidResponse)
}
impl WorklogId {
    pub fn parse(value: u64) -> Result<Self, Error> {
        positive(value).map(Self)
    }
    pub fn get(self) -> u64 {
        self.0
    }
}
impl IssueId {
    pub fn parse(value: u64) -> Result<Self, Error> {
        positive(value).map(Self)
    }
    pub fn get(self) -> u64 {
        self.0
    }
}
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AccountId(String);
impl AccountId {
    pub fn parse(value: &str) -> Result<Self, Error> {
        if value.trim().is_empty() || !bounded(value, 256, false) {
            return Err(Error::InvalidResponse);
        }
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Private worklog and account data deliberately have no Debug or Serialize.
#[derive(Clone, PartialEq, Eq)]
pub struct Attribute {
    pub key: String,
    pub value: Option<String>,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Worklog {
    pub id: WorklogId,
    pub issue: Option<IssueId>,
    pub author: AccountId,
    pub seconds: u64,
    pub billable_seconds: Option<i64>,
    pub date: NaiveDate,
    pub start_time: Option<NaiveTime>,
    pub start_instant: Option<DateTime<Utc>>,
    pub description: Option<String>,
    /// None means unexpanded, not an empty list to send back during an edit.
    pub attributes: Option<Vec<Attribute>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
impl Worklog {
    pub fn billing_account(&self) -> Result<Option<&str>, Error> {
        self.attributes
            .as_ref()
            .ok_or(Error::AttributesUnavailable)
            .map(|values| {
                values
                    .iter()
                    .find(|attribute| attribute.key == "_Account_")
                    .and_then(|attribute| attribute.value.as_deref())
            })
    }
}
pub struct Page {
    pub rows: Vec<Worklog>,
    pub offset: usize,
    pub limit: usize,
    /// An untrusted reference. Transport must validate its origin and scope.
    pub next: Option<String>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScheduleKind {
    Working,
    NonWorking,
    Holiday,
}
#[derive(Clone)]
pub struct DaySchedule {
    pub date: NaiveDate,
    pub required_seconds: u64,
    pub kind: ScheduleKind,
    pub holiday_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawWorklog {
    tempo_worklog_id: u64,
    issue: RawIssue,
    author: RawAuthor,
    time_spent_seconds: u64,
    billable_seconds: Option<i64>,
    start_date: String,
    start_time: Option<String>,
    start_date_time_utc: Option<String>,
    description: Option<String>,
    attributes: RawAttributes,
    created_at: String,
    updated_at: String,
    #[serde(rename = "self")]
    reference: String,
}
#[derive(Deserialize)]
struct RawIssue {
    id: Option<u64>,
    #[serde(rename = "self")]
    reference: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAuthor {
    account_id: String,
}
#[derive(Deserialize)]
struct RawAttributes {
    #[serde(rename = "self")]
    reference: String,
    values: Option<Vec<RawAttribute>>,
}
#[derive(Deserialize)]
struct RawAttribute {
    key: String,
    value: Option<String>,
}
#[derive(Deserialize)]
struct RawMetadata {
    count: usize,
    limit: usize,
    offset: usize,
    next: Option<String>,
}
#[derive(Deserialize)]
struct RawPage {
    metadata: RawMetadata,
    results: Vec<RawWorklog>,
    #[serde(rename = "self")]
    reference: String,
}

pub(super) fn date(text: &str) -> Result<NaiveDate, Error> {
    if text.len() != 10 {
        return Err(Error::InvalidResponse);
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .filter(|value| {
            (1..=9999).contains(&value.year()) && value.format("%Y-%m-%d").to_string() == text
        })
        .ok_or(Error::InvalidResponse)
}
fn instant(text: &str) -> Result<DateTime<Utc>, Error> {
    if text.len() > 64 {
        return Err(Error::InvalidResponse);
    }
    DateTime::parse_from_rfc3339(text)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| Error::InvalidResponse)
}
fn reference(value: &str) -> bool {
    !value.is_empty() && bounded(value, 8192, false)
}
fn attributes(raw: RawAttributes) -> Result<Option<Vec<Attribute>>, Error> {
    if !reference(&raw.reference) {
        return Err(Error::InvalidResponse);
    }
    let Some(values) = raw.values else {
        return Ok(None);
    };
    if values.len() > 256 {
        return Err(Error::InvalidResponse);
    }
    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        if value.key.is_empty()
            || !bounded(&value.key, 256, false)
            || !seen.insert(value.key.clone())
            || value
                .value
                .as_ref()
                .is_some_and(|text| !bounded(text, 16 * 1024, true))
        {
            return Err(Error::InvalidResponse);
        }
        result.push(Attribute {
            key: value.key,
            value: value.value,
        });
    }
    Ok(Some(result))
}
fn worklog(raw: RawWorklog) -> Result<Worklog, Error> {
    if !reference(&raw.reference)
        || !reference(&raw.issue.reference)
        || raw
            .description
            .as_ref()
            .is_some_and(|text| !bounded(text, 128 * 1024, true))
    {
        return Err(Error::InvalidResponse);
    }
    let start_time = raw
        .start_time
        .as_deref()
        .map(|value| {
            if value.len() > 32 {
                return Err(Error::InvalidResponse);
            }
            NaiveTime::parse_from_str(value, "%H:%M:%S%.f").map_err(|_| Error::InvalidResponse)
        })
        .transpose()?;
    Ok(Worklog {
        id: WorklogId::parse(raw.tempo_worklog_id)?,
        issue: raw.issue.id.map(IssueId::parse).transpose()?,
        author: AccountId::parse(&raw.author.account_id)?,
        seconds: positive(raw.time_spent_seconds)?,
        billable_seconds: raw.billable_seconds,
        date: date(&raw.start_date)?,
        start_time,
        start_instant: raw
            .start_date_time_utc
            .as_deref()
            .map(instant)
            .transpose()?,
        description: raw.description,
        attributes: attributes(raw.attributes)?,
        created_at: instant(&raw.created_at)?,
        updated_at: instant(&raw.updated_at)?,
    })
}
pub fn parse_worklog(bytes: &[u8]) -> Result<Worklog, Error> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(Error::TooLarge);
    }
    worklog(serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?)
}

pub fn parse_worklog_attributes(bytes: &[u8], id: WorklogId) -> Result<Vec<Attribute>, Error> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(Error::TooLarge);
    }
    #[derive(Deserialize)]
    struct Results {
        metadata: Count,
        results: Vec<RawAttribute>,
        #[serde(rename = "self")]
        reference: String,
    }
    let raw: Results = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    let reference = reqwest::Url::parse(&raw.reference).map_err(|_| Error::InvalidResponse)?;
    let expected = format!(
        "https://api.tempo.io/4/worklogs/{}/work-attribute-values",
        id.get()
    );
    if reference.as_str().trim_end_matches('/') != expected {
        return Err(Error::WrongScope);
    }
    if raw.metadata.count != raw.results.len() {
        return Err(Error::InvalidResponse);
    }
    attributes(RawAttributes {
        reference: raw.reference,
        values: Some(raw.results),
    })?
    .ok_or(Error::AttributesUnavailable)
}

pub fn parse_worklogs(bytes: &[u8]) -> Result<Page, Error> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(Error::TooLarge);
    }
    let raw: RawPage = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    let metadata = raw.metadata;
    if !reference(&raw.reference)
        || raw.results.len() > MAX_PAGE_ITEMS
        || metadata.count != raw.results.len()
        || metadata.limit == 0
        || metadata.limit > MAX_PAGE_ITEMS
        || metadata.count > metadata.limit
        || metadata.offset > i32::MAX as usize
        || metadata
            .next
            .as_ref()
            .is_some_and(|value| !reference(value))
    {
        return Err(Error::InvalidResponse);
    }
    let mut seen = HashSet::new();
    let mut rows = Vec::with_capacity(raw.results.len());
    for raw in raw.results {
        let row = worklog(raw)?;
        if !seen.insert(row.id) {
            return Err(Error::InvalidResponse);
        }
        rows.push(row);
    }
    Ok(Page {
        rows,
        offset: metadata.offset,
        limit: metadata.limit,
        next: metadata.next,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDay {
    date: String,
    required_seconds: u64,
    #[serde(rename = "type")]
    kind: String,
    holiday: Option<RawHoliday>,
}
#[derive(Deserialize)]
struct RawHoliday {
    name: String,
}
#[derive(Deserialize)]
struct Count {
    count: usize,
}
#[derive(Deserialize)]
struct RawSchedule {
    metadata: Count,
    results: Vec<RawDay>,
    #[serde(rename = "self")]
    reference: String,
}
pub fn parse_schedule(bytes: &[u8]) -> Result<Vec<DaySchedule>, Error> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(Error::TooLarge);
    }
    let raw: RawSchedule = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    if !reference(&raw.reference)
        || raw.results.len() > 366
        || raw.metadata.count != raw.results.len()
    {
        return Err(Error::InvalidResponse);
    }
    let mut seen = HashSet::new();
    raw.results
        .into_iter()
        .map(|row| {
            let date = date(&row.date)?;
            let kind = match row.kind.as_str() {
                "WORKING_DAY" => ScheduleKind::Working,
                "NON_WORKING_DAY" => ScheduleKind::NonWorking,
                "HOLIDAY" => ScheduleKind::Holiday,
                _ => return Err(Error::InvalidResponse),
            };
            if !seen.insert(date) || row.required_seconds > i64::MAX as u64 {
                return Err(Error::InvalidResponse);
            }
            let holiday_name = row.holiday.map(|holiday| holiday.name);
            if holiday_name
                .as_ref()
                .is_some_and(|name| name.is_empty() || !bounded(name, 4096, false))
            {
                return Err(Error::InvalidResponse);
            }
            Ok(DaySchedule {
                date,
                required_seconds: row.required_seconds,
                kind,
                holiday_name,
            })
        })
        .collect()
}
