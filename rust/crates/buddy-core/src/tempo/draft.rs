//! Captured Tempo create/update/delete drafts. This module never sends a request.
use std::collections::HashSet;

use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, Timelike, Utc};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{AccountId, Attribute, Error, IssueId, Worklog, WorklogId, bounded};

const MAX_WRITE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftKind {
    Create,
    Update,
    Delete,
}

enum Operation {
    Create(IssueId),
    Update(Box<Worklog>),
    Delete(Box<Worklog>),
}

/// No Debug or Serialize: a draft contains private work descriptions and IDs.
/// Author and issue cannot be changed through this draft. A future transport
/// must additionally bind it to the captured connection before sending.
pub struct Draft {
    operation: Operation,
    author: AccountId,
    date: NaiveDate,
    seconds: u64,
    billable_seconds: Option<i64>,
    start_time: Option<NaiveTime>,
    description: Option<String>,
    attributes: Vec<Attribute>,
}
impl Draft {
    pub(super) fn baseline(&self) -> Option<&Worklog> {
        match &self.operation {
            Operation::Create(_) => None,
            Operation::Update(row) | Operation::Delete(row) => Some(row),
        }
    }
    pub(super) fn recover_body(
        kind: DraftKind,
        baseline: Option<&Worklog>,
        body: &[u8],
    ) -> Result<Self, Error> {
        if kind == DraftKind::Delete {
            return match baseline {
                Some(row) if body.is_empty() => Self::delete(row),
                _ => Err(Error::InvalidRecovery),
            };
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Input {
            author_account_id: String,
            issue_id: Option<u64>,
            start_date: String,
            time_spent_seconds: u64,
            billable_seconds: Option<i64>,
            start_time: Option<String>,
            description: Option<String>,
            attributes: Vec<InputAttribute>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct InputAttribute {
            key: String,
            value: Option<String>,
        }
        let input: Input = serde_json::from_slice(body).map_err(|_| Error::InvalidRecovery)?;
        let author = AccountId::parse(&input.author_account_id)?;
        let date = super::model::date(&input.start_date)?;
        let mut draft = match (kind, baseline, input.issue_id) {
            (DraftKind::Create, None, Some(issue)) => Self::create(
                author,
                IssueId::parse(issue)?,
                date,
                input.time_spent_seconds,
            )?,
            (DraftKind::Update, Some(row), None) if row.author == author => Self::edit(row)?,
            _ => return Err(Error::InvalidRecovery),
        };
        draft.date = date;
        draft.seconds = input.time_spent_seconds;
        draft.billable_seconds = input.billable_seconds;
        draft.start_time = input
            .start_time
            .map(|value| {
                let time = NaiveTime::parse_from_str(&value, "%H:%M:%S")
                    .map_err(|_| Error::InvalidRecovery)?;
                if time.format("%H:%M:%S").to_string() != value {
                    return Err(Error::InvalidRecovery);
                }
                Ok(time)
            })
            .transpose()?;
        draft.description = input.description;
        draft.attributes = input
            .attributes
            .into_iter()
            .map(|value| Attribute {
                key: value.key,
                value: value.value,
            })
            .collect();
        if draft.encode()? != body {
            return Err(Error::InvalidRecovery);
        }
        Ok(draft)
    }
    pub fn create(
        author: AccountId,
        issue: IssueId,
        date: NaiveDate,
        seconds: u64,
    ) -> Result<Self, Error> {
        let draft = Self {
            operation: Operation::Create(issue),
            author,
            date,
            seconds,
            billable_seconds: None,
            start_time: None,
            description: None,
            attributes: Vec::new(),
        };
        draft.validate()?;
        Ok(draft)
    }
    pub fn edit(row: &Worklog) -> Result<Self, Error> {
        let draft = Self {
            operation: Operation::Update(Box::new(row.clone())),
            author: row.author.clone(),
            date: row.date,
            seconds: row.seconds,
            billable_seconds: row.billable_seconds,
            start_time: row.start_time,
            description: row.description.clone(),
            attributes: row.attributes.clone().ok_or(Error::AttributesUnavailable)?,
        };
        draft.validate()?;
        Ok(draft)
    }
    /// Copy a fully expanded record into a CREATE. No update baseline or
    /// original worklog ID can accidentally turn this into an edit.
    pub fn copy_to_date(row: &Worklog, date: NaiveDate, start: NaiveTime) -> Result<Self, Error> {
        let mut draft = Self::edit(row)?;
        draft.operation = Operation::Create(row.issue.ok_or(Error::InvalidDraft)?);
        draft.date = date;
        draft.start_time = Some(start);
        draft.validate()?;
        Ok(draft)
    }
    pub fn author(&self) -> &AccountId {
        &self.author
    }
    /// Capture the complete displayed record. Deletion has no provider body
    /// and does not require loading attributes that will not be rewritten.
    pub fn delete(row: &Worklog) -> Result<Self, Error> {
        let draft = Self {
            operation: Operation::Delete(Box::new(row.clone())),
            author: row.author.clone(),
            date: row.date,
            seconds: row.seconds,
            billable_seconds: row.billable_seconds,
            start_time: row.start_time,
            description: row.description.clone(),
            attributes: row.attributes.clone().unwrap_or_default(),
        };
        draft.validate()?;
        Ok(draft)
    }
    pub fn kind(&self) -> DraftKind {
        match self.operation {
            Operation::Create(_) => DraftKind::Create,
            Operation::Update(_) => DraftKind::Update,
            Operation::Delete(_) => DraftKind::Delete,
        }
    }
    pub fn work_date(&self) -> NaiveDate {
        self.date
    }
    pub fn worklog_id(&self) -> Option<WorklogId> {
        match &self.operation {
            Operation::Create(_) => None,
            Operation::Update(row) | Operation::Delete(row) => Some(row.id),
        }
    }
    pub fn issue_id(&self) -> Option<IssueId> {
        match &self.operation {
            Operation::Create(id) => Some(*id),
            Operation::Update(row) | Operation::Delete(row) => row.issue,
        }
    }
    /// The complete baseline is retained; this timestamp alone cannot establish
    /// freshness when a service returns changed fields with the same timestamp.
    pub fn observed_update(&self) -> Option<DateTime<Utc>> {
        match &self.operation {
            Operation::Create(_) => None,
            Operation::Update(row) | Operation::Delete(row) => Some(row.updated_at),
        }
    }
    /// Compare a scoped fresh read before an update or deletion. This detects observed
    /// changes; it is not an atomic server-side compare-and-swap guarantee.
    pub fn check_update_baseline(&self, current: &Worklog) -> Result<(), Error> {
        match &self.operation {
            Operation::Update(original) | Operation::Delete(original)
                if original.as_ref() == current =>
            {
                Ok(())
            }
            Operation::Update(_) | Operation::Delete(_) => Err(Error::ChangedWhileLoading),
            Operation::Create(_) => Err(Error::InvalidDraft),
        }
    }
    pub fn set_duration(&mut self, seconds: u64) -> Result<(), Error> {
        duration(seconds)?;
        self.seconds = seconds;
        Ok(())
    }
    pub fn set_date(&mut self, date: NaiveDate) -> Result<(), Error> {
        calendar(date)?;
        self.date = date;
        Ok(())
    }
    pub fn set_start_time(&mut self, time: Option<NaiveTime>) -> Result<(), Error> {
        if time.is_some_and(|time| time.nanosecond() != 0) {
            return Err(Error::InvalidDraft);
        }
        self.start_time = time;
        Ok(())
    }
    pub fn set_description(&mut self, text: Option<String>) -> Result<(), Error> {
        if text
            .as_ref()
            .is_some_and(|value| !bounded(value, 128 * 1024, true))
        {
            return Err(Error::InvalidDraft);
        }
        self.description = text;
        Ok(())
    }
    pub fn set_billable_seconds(&mut self, value: Option<i64>) {
        self.billable_seconds = value;
    }
    pub fn set_billing_account(&mut self, value: Option<&str>) -> Result<(), Error> {
        if value.is_some_and(|value| value.trim().is_empty() || !bounded(value, 16 * 1024, false)) {
            return Err(Error::InvalidDraft);
        }
        if let Some(value) = value {
            if let Some(attribute) = self
                .attributes
                .iter_mut()
                .find(|attribute| attribute.key == "_Account_")
            {
                attribute.value = Some(value.into());
            } else {
                if self.attributes.len() >= 256 {
                    return Err(Error::InvalidDraft);
                }
                self.attributes.push(Attribute {
                    key: "_Account_".into(),
                    value: Some(value.into()),
                });
            }
        } else {
            self.attributes
                .retain(|attribute| attribute.key != "_Account_");
        }
        Ok(())
    }
    fn validate(&self) -> Result<(), Error> {
        calendar(self.date)?;
        duration(self.seconds)?;
        if self.start_time.is_some_and(|time| time.nanosecond() != 0)
            || self
                .description
                .as_ref()
                .is_some_and(|value| !bounded(value, 128 * 1024, true))
            || self.attributes.len() > 256
        {
            return Err(Error::InvalidDraft);
        }
        let mut keys = HashSet::new();
        for attribute in &self.attributes {
            if attribute.key.is_empty()
                || !bounded(&attribute.key, 256, false)
                || !keys.insert(&attribute.key)
                || attribute
                    .value
                    .as_ref()
                    .is_some_and(|value| !bounded(value, 16 * 1024, true))
            {
                return Err(Error::InvalidDraft);
            }
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        if matches!(self.operation, Operation::Delete(_)) {
            return Ok(Vec::new());
        }
        let mut fields = Map::new();
        fields.insert("authorAccountId".into(), json!(self.author.as_str()));
        fields.insert("startDate".into(), json!(self.date.to_string()));
        fields.insert("timeSpentSeconds".into(), json!(self.seconds));
        if let Operation::Create(issue) = self.operation {
            fields.insert("issueId".into(), json!(issue.get()));
        }
        if let Some(seconds) = self.billable_seconds {
            fields.insert("billableSeconds".into(), json!(seconds));
        }
        if let Some(time) = self.start_time {
            fields.insert(
                "startTime".into(),
                json!(time.format("%H:%M:%S").to_string()),
            );
        }
        if let Some(text) = &self.description {
            fields.insert("description".into(), json!(text));
        }
        let attributes = self
            .attributes
            .iter()
            .map(|attribute| {
                let mut fields = Map::new();
                fields.insert("key".into(), json!(attribute.key));
                if let Some(value) = &attribute.value {
                    fields.insert("value".into(), json!(value));
                }
                Value::Object(fields)
            })
            .collect::<Vec<_>>();
        fields.insert("attributes".into(), Value::Array(attributes));
        let bytes = serde_json::to_vec(&fields).map_err(|_| Error::InvalidDraft)?;
        if bytes.len() > MAX_WRITE_BYTES {
            return Err(Error::DraftTooLarge);
        }
        Ok(bytes)
    }
}
fn calendar(date: NaiveDate) -> Result<(), Error> {
    (1..=9999)
        .contains(&date.year())
        .then_some(())
        .ok_or(Error::InvalidDraft)
}
fn duration(seconds: u64) -> Result<(), Error> {
    (seconds > 0 && seconds <= i64::MAX as u64)
        .then_some(())
        .ok_or(Error::InvalidDraft)
}

#[cfg(test)]
#[path = "draft_tests.rs"]
mod tests;
