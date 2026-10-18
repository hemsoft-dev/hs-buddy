//! Pending recovery is reconciliation only. Terminal acknowledgements remain
//! terminal, and no restored value can become a ready WriteAttempt.
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    Client, Draft, DraftKind, Error, JiraClient, JiraError, MAX_RESPONSE_BYTES, Worklog, WorklogId,
    WriteAttempt, WritePhase, WriteRequest, parse_worklog,
};

const MAX_RECOVERY_BYTES: usize = MAX_RESPONSE_BYTES * 4 + 8192;
pub(super) const MAX_STORED_BYTES: usize = MAX_RECOVERY_BYTES + 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryError {
    Identity(JiraError),
    Domain(Error),
}
impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identity(error) => error.fmt(f),
            Self::Domain(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for RecoveryError {}
impl From<Error> for RecoveryError {
    fn from(error: Error) -> Self {
        Self::Domain(error)
    }
}

struct Context {
    profile: String,
    origin: String,
    account: String,
}
impl Context {
    async fn current(config: &Path, jira: &JiraClient) -> Result<Self, RecoveryError> {
        let profile = crate::pollen_credentials::account_for_path(config)
            .map_err(|_| Error::InvalidRecovery)?;
        let account = jira
            .myself()
            .await
            .map_err(RecoveryError::Identity)?
            .account
            .as_str()
            .to_owned();
        Ok(Self {
            profile,
            origin: jira.origin().into(),
            account,
        })
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u8,
    profile: String,
    origin: String,
    account: String,
    operation: String,
    kind: String,
    target: Option<u64>,
    body: String,
    baseline: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<Terminal>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum Terminal {
    Confirmed { worklog_id: u64 },
    Rejected {},
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Acknowledgement {
    version: u8,
    operation: String,
    checkpoint_sha256: String,
    outcome: Terminal,
}

pub(super) fn acknowledgement(
    attempt: &WriteAttempt,
    scope: super::WriteScope,
    checkpoint: &[u8],
) -> Result<Vec<u8>, Error> {
    let request = attempt.terminal_request(scope)?;
    if checkpoint.is_empty() || checkpoint.len() > MAX_RECOVERY_BYTES {
        return Err(Error::InvalidRecovery);
    }
    let stored: Stored = serde_json::from_slice(checkpoint).map_err(|_| Error::InvalidRecovery)?;
    let kind = match request.draft().kind() {
        DraftKind::Create => "create",
        DraftKind::Update => "update",
        DraftKind::Delete => "delete",
    };
    if stored.version != 1
        || stored.outcome.is_some()
        || stored.operation != request.operation_id().to_string()
        || stored.body.as_bytes() != request.body()
        || stored.kind != kind
        || stored.target != request.draft().worklog_id().map(|id| id.get())
        || stored.account != request.draft().author().as_str()
        || stored.baseline != request.draft().baseline().map(wire)
    {
        return Err(Error::InvalidRecovery);
    }
    let marker = Acknowledgement {
        version: 1,
        operation: request.operation_id().to_string(),
        checkpoint_sha256: format!("{:x}", Sha256::digest(checkpoint)),
        outcome: Terminal::capture(attempt)?,
    };
    let bytes = serde_json::to_vec(&marker).map_err(|_| Error::InvalidRecovery)?;
    if bytes.len() > 4096 {
        return Err(Error::InvalidRecovery);
    }
    Ok(bytes)
}
impl Terminal {
    fn capture(attempt: &WriteAttempt) -> Result<Self, Error> {
        match (attempt.phase(), attempt.confirmed_id()) {
            (WritePhase::Confirmed, Some(id)) => Ok(Self::Confirmed {
                worklog_id: id.get(),
            }),
            (WritePhase::Rejected, None) => Ok(Self::Rejected {}),
            _ => Err(Error::WriteBlocked),
        }
    }
    fn validate(&self, draft: &Draft) -> Result<(), Error> {
        if let Self::Confirmed { worklog_id } = self {
            let id = WorklogId::parse(*worklog_id).map_err(|_| Error::InvalidRecovery)?;
            if draft.kind() != DraftKind::Create && draft.worklog_id() != Some(id) {
                return Err(Error::InvalidRecovery);
            }
        }
        Ok(())
    }
}

/// No Debug or dispatch method. The local UUID is correlation only. Legacy and
/// pending records remain Unconfirmed; acknowledged records permit local cleanup.
pub struct RecoveredChange {
    operation: Uuid,
    draft: Draft,
    body: Vec<u8>,
    outcome: Option<Terminal>,
}
impl RecoveredChange {
    pub fn operation_id(&self) -> Uuid {
        self.operation
    }
    pub fn phase(&self) -> WritePhase {
        match self.outcome {
            Some(Terminal::Confirmed { .. }) => WritePhase::Confirmed,
            Some(Terminal::Rejected {}) => WritePhase::Rejected,
            None => WritePhase::Unconfirmed,
        }
    }
    pub fn confirmed_id(&self) -> Option<WorklogId> {
        match self.outcome {
            Some(Terminal::Confirmed { worklog_id }) => WorklogId::parse(worklog_id).ok(),
            _ => None,
        }
    }
    pub fn draft(&self) -> &Draft {
        &self.draft
    }
    pub fn original_body(&self) -> &[u8] {
        &self.body
    }
}

impl Client {
    /// Encode only. The app must atomically persist this record before dispatch.
    pub async fn checkpoint(
        &self,
        attempt: &WriteAttempt,
        config: &Path,
        jira: &JiraClient,
    ) -> Result<Vec<u8>, RecoveryError> {
        let request = attempt.pending_request(self.write_scope())?;
        encode(request, &Context::current(config, jira).await?).map_err(Into::into)
    }

    /// Encode an already observed terminal outcome for durable cleanup recovery.
    /// The caller must persist it; this method never dispatches or retries a write.
    pub async fn terminal_checkpoint(
        &self,
        attempt: &WriteAttempt,
        config: &Path,
        jira: &JiraClient,
    ) -> Result<Vec<u8>, RecoveryError> {
        let request = attempt.terminal_request(self.write_scope())?;
        let outcome = Terminal::capture(attempt)?;
        encode_with_outcome(
            request,
            &Context::current(config, jira).await?,
            Some(outcome),
        )
        .map_err(Into::into)
    }
}

/// Re-read the captured Jira identity; never send a worklog while opening it.
pub async fn inspect(
    bytes: &[u8],
    config: &Path,
    jira: &JiraClient,
) -> Result<RecoveredChange, RecoveryError> {
    if bytes.len() > MAX_STORED_BYTES {
        return Err(Error::InvalidRecovery.into());
    }
    decode(bytes, &Context::current(config, jira).await?).map_err(Into::into)
}
fn encode(request: &WriteRequest, context: &Context) -> Result<Vec<u8>, Error> {
    encode_with_outcome(request, context, None)
}
fn encode_with_outcome(
    request: &WriteRequest,
    context: &Context,
    outcome: Option<Terminal>,
) -> Result<Vec<u8>, Error> {
    if request.draft().author().as_str() != context.account {
        return Err(Error::WrongConnection);
    }
    if let Some(outcome) = &outcome {
        outcome.validate(request.draft())?;
    }
    let stored = Stored {
        version: 1,
        profile: context.profile.clone(),
        origin: context.origin.clone(),
        account: context.account.clone(),
        operation: request.operation_id().to_string(),
        kind: match request.draft().kind() {
            DraftKind::Create => "create",
            DraftKind::Update => "update",
            DraftKind::Delete => "delete",
        }
        .into(),
        target: request.draft().worklog_id().map(|id| id.get()),
        body: std::str::from_utf8(request.body())
            .map_err(|_| Error::InvalidRecovery)?
            .into(),
        baseline: request.draft().baseline().map(wire),
        outcome,
    };
    let bytes = serde_json::to_vec(&stored).map_err(|_| Error::InvalidRecovery)?;
    if bytes.len() > MAX_RECOVERY_BYTES {
        return Err(Error::InvalidRecovery);
    }
    Ok(bytes)
}
fn decode(bytes: &[u8], context: &Context) -> Result<RecoveredChange, Error> {
    if bytes.len() > MAX_STORED_BYTES {
        return Err(Error::InvalidRecovery);
    }
    let mut stream = serde_json::Deserializer::from_slice(bytes).into_iter::<Stored>();
    let mut stored = stream
        .next()
        .ok_or(Error::InvalidRecovery)?
        .map_err(|_| Error::InvalidRecovery)?;
    let offset = stream.byte_offset();
    if offset > MAX_RECOVERY_BYTES {
        return Err(Error::InvalidRecovery);
    }
    let trailing = &bytes[offset..];
    if !trailing.iter().all(u8::is_ascii_whitespace) {
        if trailing.len() > 4096 || stored.outcome.is_some() {
            return Err(Error::InvalidRecovery);
        }
        let marker: Acknowledgement =
            serde_json::from_slice(trailing).map_err(|_| Error::InvalidRecovery)?;
        if marker.version != 1
            || marker.operation != stored.operation
            || marker.checkpoint_sha256 != format!("{:x}", Sha256::digest(&bytes[..offset]))
        {
            return Err(Error::InvalidRecovery);
        }
        stored.outcome = Some(marker.outcome);
    }
    if stored.version != 1 {
        return Err(Error::InvalidRecovery);
    }
    if stored.profile != context.profile
        || stored.origin != context.origin
        || stored.account != context.account
    {
        return Err(Error::WrongConnection);
    }
    let operation = Uuid::parse_str(&stored.operation).map_err(|_| Error::InvalidRecovery)?;
    if operation.get_version_num() != 4 || operation.to_string() != stored.operation {
        return Err(Error::InvalidRecovery);
    }
    let kind = match stored.kind.as_str() {
        "create" => DraftKind::Create,
        "update" => DraftKind::Update,
        "delete" => DraftKind::Delete,
        _ => return Err(Error::InvalidRecovery),
    };
    let baseline = stored
        .baseline
        .as_ref()
        .map(|value| {
            let bytes = serde_json::to_vec(value).map_err(|_| Error::InvalidRecovery)?;
            let row = parse_worklog(&bytes).map_err(|_| Error::InvalidRecovery)?;
            if wire(&row) != *value {
                return Err(Error::InvalidRecovery);
            }
            Ok(row)
        })
        .transpose()?;
    let draft = Draft::recover_body(kind, baseline.as_ref(), stored.body.as_bytes())
        .map_err(|_| Error::InvalidRecovery)?;
    if draft.author().as_str() != context.account
        || draft.worklog_id().map(|id| id.get()) != stored.target
    {
        return Err(Error::InvalidRecovery);
    }
    if let Some(outcome) = &stored.outcome {
        outcome.validate(&draft)?;
    }
    Ok(RecoveredChange {
        operation,
        draft,
        body: stored.body.into_bytes(),
        outcome: stored.outcome,
    })
}
fn wire(row: &Worklog) -> Value {
    let id = row.id.get();
    json!({"tempoWorklogId":id, "self":format!("https://api.tempo.io/4/worklogs/{id}"),
        "issue":{"id":row.issue.map(|id| id.get()), "self":"https://api.tempo.io/4/issues"},
        "author":{"accountId":row.author.as_str()}, "timeSpentSeconds":row.seconds, "billableSeconds":row.billable_seconds,
        "startDate":row.date.to_string(), "startTime":row.start_time.map(|time| time.format("%H:%M:%S").to_string()),
        "startDateTimeUtc":row.start_instant.map(|time| time.to_rfc3339()), "description":row.description,
        "createdAt":row.created_at.to_rfc3339(), "updatedAt":row.updated_at.to_rfc3339(),
        "attributes":{"self":format!("https://api.tempo.io/4/worklogs/{id}/work-attribute-values"),
            "values":row.attributes.as_ref().map(|values| values.iter().map(|value| json!({"key":value.key,"value":value.value})).collect::<Vec<_>>())}})
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
