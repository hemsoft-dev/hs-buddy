//! Profile-local recovery keeps the original command bytes and UUIDs. It never
//! stores credentials or sends a command automatically.
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::*;

use super::super::MAX_CHECKPOINT_BYTES;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u8,
    profile: String,
    account: String,
    commands: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Acknowledgement {
    version: u8,
    checkpoint_sha256: String,
    command_id: String,
    outcome: AcknowledgedOutcome,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum AcknowledgedOutcome {
    Confirmed { task: String },
    Rejected,
}

fn record(bytes: &[u8]) -> Result<(Checkpoint, Option<Acknowledgement>), Error> {
    if bytes.len() > MAX_CHECKPOINT_BYTES {
        return Err(Error::InvalidRecovery);
    }
    // One original checkpoint, optionally followed by one durable outcome.
    // A partial or additional outcome is rejected rather than replayed.
    let mut stream = serde_json::Deserializer::from_slice(bytes).into_iter::<Checkpoint>();
    let saved = stream
        .next()
        .ok_or(Error::InvalidRecovery)?
        .map_err(|_| Error::InvalidRecovery)?;
    let remainder = &bytes[stream.byte_offset()..];
    let acknowledgement = if remainder.iter().all(u8::is_ascii_whitespace) {
        None
    } else {
        Some(serde_json::from_slice(remainder).map_err(|_| Error::InvalidRecovery)?)
    };
    if saved.version != 1 {
        return Err(Error::InvalidRecovery);
    }
    Ok((saved, acknowledgement))
}

fn fingerprint(saved: &Checkpoint) -> Result<String, Error> {
    let bytes = serde_json::to_vec(saved).map_err(|_| Error::InvalidRecovery)?;
    Ok(crate::sha256::hex_digest(bytes))
}

impl Acknowledgement {
    fn restore(
        &self,
        saved: &Checkpoint,
        request: &WriteRequest,
    ) -> Result<(WritePhase, Option<Id>), Error> {
        if self.version != 1
            || self.command_id != request.command_id().to_string()
            || self.checkpoint_sha256 != fingerprint(saved)?
        {
            return Err(Error::InvalidRecovery);
        }
        match &self.outcome {
            AcknowledgedOutcome::Confirmed { task } => {
                let id = Id::parse(task).map_err(|_| Error::InvalidRecovery)?;
                if request.kind != WriteKind::Create && request.target.as_ref() != Some(&id) {
                    return Err(Error::InvalidRecovery);
                }
                Ok((WritePhase::Confirmed, Some(id)))
            }
            AcknowledgedOutcome::Rejected => Ok((WritePhase::Rejected, None)),
        }
    }
}

fn profile(path: &Path) -> Result<String, Error> {
    crate::pollen_credentials::account_for_path(path).map_err(|_| Error::InvalidRecovery)
}

impl WriteAttempt {
    /// An append-only outcome keeps the original command bytes intact. No
    /// account read or service request is needed after a definite response.
    pub(in super::super) fn acknowledgement(&self, bytes: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        if !matches!(self.phase, WritePhase::Confirmed | WritePhase::Rejected) {
            return Err(Error::WriteBlocked);
        }
        let (saved, existing) = record(bytes)?;
        if saved.commands != self.request.commands {
            return Err(Error::InvalidRecovery);
        }
        if let Some(existing) = existing {
            let (phase, task) = existing.restore(&saved, &self.request)?;
            if phase != self.phase || task != self.task {
                return Err(Error::InvalidRecovery);
            }
            return Ok(None);
        }
        let outcome = match self.phase {
            WritePhase::Confirmed => AcknowledgedOutcome::Confirmed {
                task: self
                    .task
                    .as_ref()
                    .ok_or(Error::InvalidRecovery)?
                    .as_str()
                    .to_owned(),
            },
            WritePhase::Rejected => AcknowledgedOutcome::Rejected,
            _ => unreachable!(),
        };
        serde_json::to_vec(&Acknowledgement {
            version: 1,
            checkpoint_sha256: fingerprint(&saved)?,
            command_id: self.request.command_id().to_string(),
            outcome,
        })
        .map(Some)
        .map_err(|_| Error::InvalidRecovery)
    }

    pub(in super::super) fn checkpoint(&self, path: &Path, account: &Id) -> Result<Vec<u8>, Error> {
        if matches!(self.phase, WritePhase::Confirmed | WritePhase::Rejected) {
            return Err(Error::WriteBlocked);
        }
        let bytes = serde_json::to_vec(&Checkpoint {
            version: 1,
            profile: profile(path)?,
            account: account.as_str().into(),
            commands: self.request.commands.clone(),
        })
        .map_err(|_| Error::InvalidRecovery)?;
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(Error::InvalidRecovery);
        }
        Ok(bytes)
    }

    pub(in super::super) fn restore(
        bytes: &[u8],
        path: &Path,
        account: &Id,
        scope: WriteScope,
    ) -> Result<Self, Error> {
        let (saved, acknowledgement) = record(bytes)?;
        if saved.profile != profile(path)? || saved.account != account.as_str() {
            return Err(Error::WrongConnection);
        }
        let request = restore_request(scope, saved.commands.clone())?;
        let (phase, task) = match acknowledgement {
            Some(acknowledgement) => acknowledgement.restore(&saved, &request)?,
            None => (WritePhase::Unconfirmed, None),
        };
        Ok(Self {
            request: Arc::new(request),
            phase,
            task,
        })
    }
}

fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str, Error> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or(Error::InvalidRecovery)
}
fn uuid(value: &Value, name: &str) -> Result<Uuid, Error> {
    let id = Uuid::parse_str(text(value, name)?).map_err(|_| Error::InvalidRecovery)?;
    if id.get_version_num() != 4 {
        return Err(Error::InvalidRecovery);
    }
    Ok(id)
}
fn optional<T>(
    args: &Value,
    name: &str,
    parse: impl FnOnce(&Value) -> Result<T, Error>,
) -> Result<Option<T>, Error> {
    args.get(name).map(parse).transpose()
}

fn restore_request(scope: WriteScope, commands: String) -> Result<WriteRequest, Error> {
    if commands.len() > MAX_MUTATION_BYTES {
        return Err(Error::InvalidRecovery);
    }
    let value: Value = serde_json::from_str(&commands).map_err(|_| Error::InvalidRecovery)?;
    if serde_json::to_string(&value).map_err(|_| Error::InvalidRecovery)? != commands {
        return Err(Error::InvalidRecovery);
    }
    let array = value
        .as_array()
        .filter(|v| v.len() == 1)
        .ok_or(Error::InvalidRecovery)?;
    let command = &array[0];
    let command_id = uuid(command, "uuid")?;
    let args = command.get("args").ok_or(Error::InvalidRecovery)?;
    let priority = |v: &Value| {
        v.as_u64()
            .and_then(|v| u8::try_from(v).ok())
            .ok_or(Error::InvalidRecovery)
            .and_then(Priority::from_api)
    };
    let date = optional(args, "due", |v| {
        NaiveDate::parse_from_str(text(v, "date")?, "%Y-%m-%d").map_err(|_| Error::InvalidRecovery)
    })?;
    let target = || Id::parse(text(args, "id")?);
    // Reuse draft validation, then compare the entire reconstructed shape.
    // Unknown fields, null values and extra commands cannot be replayed.
    let mut request = match text(command, "type")? {
        "item_add" => WriteRequest::create(
            scope,
            &CreateDraft {
                content: text(args, "content")?.into(),
                description: text(args, "description")?.into(),
                project: optional(args, "project_id", |v| {
                    Id::parse(v.as_str().ok_or(Error::InvalidRecovery)?)
                })?,
                priority: priority(args.get("priority").ok_or(Error::InvalidRecovery)?)?,
                due: date,
            },
        ),
        "item_update" => WriteRequest::edit(
            scope,
            &target()?,
            &EditDraft {
                content: optional(args, "content", |v| {
                    v.as_str().map(str::to_owned).ok_or(Error::InvalidRecovery)
                })?,
                description: optional(args, "description", |v| {
                    v.as_str().map(str::to_owned).ok_or(Error::InvalidRecovery)
                })?,
                priority: optional(args, "priority", priority)?,
                due: date,
            },
        ),
        "item_close" => WriteRequest::complete(scope, &target()?),
        "item_uncomplete" => WriteRequest::reopen(scope, &target()?),
        "item_delete" => WriteRequest::delete(scope, &target()?),
        _ => return Err(Error::InvalidRecovery),
    }
    .map_err(|_| Error::InvalidRecovery)?;
    let temporary_id = if request.kind == WriteKind::Create {
        let id = uuid(command, "temp_id")?;
        if id == command_id {
            return Err(Error::InvalidRecovery);
        }
        Some(id)
    } else {
        None
    };
    let mut expected: Value =
        serde_json::from_str(&request.commands).map_err(|_| Error::InvalidRecovery)?;
    expected[0]["uuid"] = command["uuid"].clone();
    if temporary_id.is_some() {
        expected[0]["temp_id"] = command["temp_id"].clone();
    }
    if expected != value {
        return Err(Error::InvalidRecovery);
    }
    request.command_id = command_id;
    request.temporary_id = temporary_id;
    request.commands = commands;
    Ok(request)
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
