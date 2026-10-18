//! Sync commands carry stable UUIDs, as documented in Todoist's Sync guide.
//! A retry reuses the captured command, rather than rebuilding an editable draft.
use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{Error, Id, MAX_MUTATION_BYTES, MAX_RESPONSE_BYTES, Priority, bounded_text};

#[path = "recovery.rs"]
mod recovery;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct WriteScope(Uuid);
impl WriteScope {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
impl Default for WriteScope {
    fn default() -> Self {
        Self::new()
    }
}

pub struct CreateDraft {
    pub content: String,
    pub description: String,
    pub project: Option<Id>,
    pub priority: Priority,
    pub due: Option<NaiveDate>,
}

/// Absent fields are preserved, including an existing timed or recurring due
/// value. Changing content must never replace its due value with a date only.
#[derive(Default)]
pub struct EditDraft {
    pub content: Option<String>,
    pub description: Option<String>,
    pub priority: Option<Priority>,
    pub due: Option<NaiveDate>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteKind {
    Create,
    Edit,
    Complete,
    Reopen,
    Delete,
}

/// No Debug or mutable payload access. Private task text stays in this snapshot.
pub struct WriteRequest {
    scope: WriteScope,
    kind: WriteKind,
    command_id: Uuid,
    temporary_id: Option<Uuid>,
    target: Option<Id>,
    commands: String,
}
pub enum WriteOutcome {
    Confirmed(Id),
    Rejected,
    Unconfirmed,
}

fn content(value: &str) -> Result<String, Error> {
    let value = value.trim();
    if value.is_empty() || !bounded_text(value, 16 * 1024, true) {
        return Err(Error::InvalidDraft);
    }
    Ok(value.into())
}
fn description(value: &str) -> Result<String, Error> {
    if !bounded_text(value, 128 * 1024, true) {
        return Err(Error::InvalidDraft);
    }
    Ok(value.into())
}
fn due(date: NaiveDate) -> Result<Value, Error> {
    if !(1..=9999).contains(&date.year()) {
        return Err(Error::InvalidDraft);
    }
    Ok(json!({"date":date.format("%Y-%m-%d").to_string()}))
}

impl WriteRequest {
    fn capture(
        scope: WriteScope,
        kind: WriteKind,
        target: Option<Id>,
        args: Map<String, Value>,
    ) -> Result<Self, Error> {
        let command_id = Uuid::new_v4();
        let temporary_id = (kind == WriteKind::Create).then(Uuid::new_v4);
        let name = match kind {
            WriteKind::Create => "item_add",
            WriteKind::Edit => "item_update",
            WriteKind::Complete => "item_close",
            WriteKind::Reopen => "item_uncomplete",
            WriteKind::Delete => "item_delete",
        };
        let mut command = json!({"type":name,"uuid":command_id.to_string(),"args":args});
        if let Some(id) = temporary_id {
            command["temp_id"] = json!(id.to_string());
        }
        let commands = serde_json::to_string(&[command]).map_err(|_| Error::InvalidDraft)?;
        if commands.len() > MAX_MUTATION_BYTES {
            return Err(Error::InvalidDraft);
        }
        Ok(Self {
            scope,
            kind,
            command_id,
            temporary_id,
            target,
            commands,
        })
    }
    pub fn create(scope: WriteScope, draft: &CreateDraft) -> Result<Self, Error> {
        let mut args = Map::new();
        args.insert("content".into(), json!(content(&draft.content)?));
        args.insert(
            "description".into(),
            json!(description(&draft.description)?),
        );
        args.insert("priority".into(), json!(draft.priority.api_value()));
        args.insert("auto_parse_labels".into(), json!(false));
        if let Some(project) = &draft.project {
            args.insert("project_id".into(), json!(project.as_str()));
        }
        if let Some(date) = draft.due {
            args.insert("due".into(), due(date)?);
        }
        Self::capture(scope, WriteKind::Create, None, args)
    }
    pub fn edit(scope: WriteScope, target: &Id, draft: &EditDraft) -> Result<Self, Error> {
        let mut args = Map::new();
        args.insert("id".into(), json!(target.as_str()));
        if let Some(value) = &draft.content {
            args.insert("content".into(), json!(content(value)?));
        }
        if let Some(value) = &draft.description {
            args.insert("description".into(), json!(description(value)?));
        }
        if let Some(priority) = draft.priority {
            args.insert("priority".into(), json!(priority.api_value()));
        }
        if let Some(date) = draft.due {
            args.insert("due".into(), due(date)?);
        }
        if args.len() == 1 {
            return Err(Error::InvalidDraft);
        }
        Self::capture(scope, WriteKind::Edit, Some(target.clone()), args)
    }
    pub fn complete(scope: WriteScope, target: &Id) -> Result<Self, Error> {
        Self::capture(
            scope,
            WriteKind::Complete,
            Some(target.clone()),
            Map::from_iter([("id".into(), json!(target.as_str()))]),
        )
    }
    pub fn reopen(scope: WriteScope, target: &Id) -> Result<Self, Error> {
        Self::capture(
            scope,
            WriteKind::Reopen,
            Some(target.clone()),
            Map::from_iter([("id".into(), json!(target.as_str()))]),
        )
    }
    pub fn delete(scope: WriteScope, target: &Id) -> Result<Self, Error> {
        Self::capture(
            scope,
            WriteKind::Delete,
            Some(target.clone()),
            Map::from_iter([("id".into(), json!(target.as_str()))]),
        )
    }
    pub fn command_id(&self) -> Uuid {
        self.command_id
    }
    pub fn temporary_id(&self) -> Option<Uuid> {
        self.temporary_id
    }
    pub fn kind(&self) -> WriteKind {
        self.kind
    }
    pub fn commands(&self) -> &str {
        &self.commands
    }
    pub fn check_scope(&self, scope: WriteScope) -> Result<(), Error> {
        if scope == self.scope {
            Ok(())
        } else {
            Err(Error::WrongConnection)
        }
    }

    pub fn decode(&self, bytes: &[u8]) -> WriteOutcome {
        if bytes.len() > MAX_RESPONSE_BYTES {
            return WriteOutcome::Unconfirmed;
        }
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            return WriteOutcome::Unconfirmed;
        };
        let Some(status) = value
            .get("sync_status")
            .and_then(|statuses| statuses.get(self.command_id.to_string()))
        else {
            return WriteOutcome::Unconfirmed;
        };
        if status.as_str() == Some("ok") {
            if let Some(temporary_id) = self.temporary_id {
                return value
                    .get("temp_id_mapping")
                    .and_then(|mapping| mapping.get(temporary_id.to_string()))
                    .and_then(Value::as_str)
                    .and_then(|id| Id::parse(id).ok())
                    .map(WriteOutcome::Confirmed)
                    .unwrap_or(WriteOutcome::Unconfirmed);
            }
            return self
                .target
                .clone()
                .map(WriteOutcome::Confirmed)
                .unwrap_or(WriteOutcome::Unconfirmed);
        }
        if status
            .get("error_code")
            .and_then(Value::as_u64)
            .is_some_and(|code| code > 0)
            && status.get("error").is_some_and(Value::is_string)
        {
            WriteOutcome::Rejected
        } else {
            WriteOutcome::Unconfirmed
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePhase {
    Ready,
    InFlight,
    Unconfirmed,
    Confirmed,
    Rejected,
}

/// One visible user change. Storage uncertainty blocks replacing its snapshot.
pub struct WriteAttempt {
    request: Arc<WriteRequest>,
    phase: WritePhase,
    task: Option<Id>,
}
impl WriteAttempt {
    pub(super) fn request_scope(&self, scope: WriteScope) -> Result<(), Error> {
        self.request.check_scope(scope)
    }
    pub fn new(request: WriteRequest) -> Self {
        Self {
            request: Arc::new(request),
            phase: WritePhase::Ready,
            task: None,
        }
    }
    pub fn phase(&self) -> WritePhase {
        self.phase
    }
    pub(super) fn request(&self) -> &WriteRequest {
        &self.request
    }
    pub fn can_replace(&self) -> bool {
        matches!(
            self.phase,
            WritePhase::Ready | WritePhase::Confirmed | WritePhase::Rejected
        )
    }
    pub fn task_id(&self) -> Option<&Id> {
        self.task.as_ref()
    }
    pub fn begin(&mut self, scope: WriteScope) -> Result<Arc<WriteRequest>, Error> {
        self.request.check_scope(scope)?;
        if !matches!(self.phase, WritePhase::Ready | WritePhase::Unconfirmed) {
            return Err(Error::WriteBlocked);
        }
        self.phase = WritePhase::InFlight;
        Ok(self.request.clone())
    }
    pub fn observe(&mut self, command_id: Uuid, outcome: WriteOutcome) -> bool {
        if self.phase != WritePhase::InFlight || command_id != self.request.command_id {
            return false;
        }
        match outcome {
            WriteOutcome::Confirmed(id) => {
                self.task = Some(id);
                self.phase = WritePhase::Confirmed;
            }
            WriteOutcome::Rejected => self.phase = WritePhase::Rejected,
            WriteOutcome::Unconfirmed => self.phase = WritePhase::Unconfirmed,
        }
        true
    }
}
