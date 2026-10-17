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

fn profile(path: &Path) -> Result<String, Error> {
    crate::pollen_credentials::account_for_path(path).map_err(|_| Error::InvalidRecovery)
}

impl WriteAttempt {
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
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(Error::InvalidRecovery);
        }
        let saved: Checkpoint =
            serde_json::from_slice(bytes).map_err(|_| Error::InvalidRecovery)?;
        if saved.version != 1 {
            return Err(Error::InvalidRecovery);
        }
        if saved.profile != profile(path)? || saved.account != account.as_str() {
            return Err(Error::WrongConnection);
        }
        let request = restore_request(scope, saved.commands)?;
        Ok(Self {
            request: Arc::new(request),
            // A crash may follow dispatch but precede recording the result.
            phase: WritePhase::Unconfirmed,
            task: None,
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
