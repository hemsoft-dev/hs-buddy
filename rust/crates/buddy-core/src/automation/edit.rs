//! Preserve an attempted configuration edit without ever replaying its mutation.
use super::backend::{AutomationBackend, ChangeKind, Error as BackendError};
use crate::pending_file::{PendingFile, RecoverySlot, StorageError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

const MAX_BYTES: usize = 512 * 1024;
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    version: u8,
    deployment: String,
    pub kind: ChangeKind,
    pub arguments: Value,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Acknowledgement {
    acknowledged: bool,
}

#[async_trait]
pub trait Service: Send + Sync {
    fn deployment(&self) -> &str;
    async fn send(&self, kind: ChangeKind, arguments: Value) -> Result<Value, BackendError>;
}
#[async_trait]
impl Service for AutomationBackend {
    fn deployment(&self) -> &str {
        self.deployment()
    }
    async fn send(&self, kind: ChangeKind, arguments: Value) -> Result<Value, BackendError> {
        self.change(kind, arguments).await
    }
}
pub use super::trigger::Error;

pub struct Edit {
    file: PendingFile,
    deployment: String,
    capture: Option<Capture>,
    acknowledged: bool,
    fresh: bool,
}
impl Edit {
    pub fn open(config: &Path, deployment: &str) -> Result<Self, Error> {
        let deployment = AutomationBackend::new(deployment)
            .map_err(|_| Error::Invalid)?
            .deployment()
            .to_owned();
        let file = PendingFile::open(config, RecoverySlot::AutomationEdit, MAX_BYTES)?;
        let mut state = Self {
            file,
            deployment,
            capture: None,
            acknowledged: false,
            fresh: false,
        };
        if let Some(bytes) = state.file.current() {
            let mut stream = serde_json::Deserializer::from_slice(bytes).into_iter::<Value>();
            let capture: Capture = serde_json::from_value(
                stream
                    .next()
                    .ok_or(Error::Invalid)?
                    .map_err(|_| Error::Invalid)?,
            )
            .map_err(|_| Error::Invalid)?;
            if capture.version != 1
                || capture.deployment != state.deployment
                || !capture.arguments.is_object()
            {
                return Err(Error::Invalid);
            }
            if let Some(value) = stream.next() {
                let ack: Acknowledgement =
                    serde_json::from_value(value.map_err(|_| Error::Invalid)?)
                        .map_err(|_| Error::Invalid)?;
                if !ack.acknowledged {
                    return Err(Error::Invalid);
                }
                state.acknowledged = true;
            }
            if stream.next().is_some() {
                return Err(Error::Invalid);
            }
            state.capture = Some(capture);
        }
        Ok(state)
    }
    pub fn capture(&self) -> Option<&Capture> {
        self.capture.as_ref()
    }
    pub fn acknowledged(&self) -> bool {
        self.acknowledged
    }
    pub fn prepare(&mut self, kind: ChangeKind, arguments: Value) -> Result<(), Error> {
        if self.capture.is_some() {
            return Err(Error::Pending);
        }
        if !arguments.is_object() {
            return Err(Error::Invalid);
        }
        let capture = Capture {
            version: 1,
            deployment: self.deployment.clone(),
            kind,
            arguments,
        };
        let bytes = serde_json::to_vec(&capture).map_err(|_| Error::Invalid)?;
        if bytes.len() > MAX_BYTES - 4096 {
            return Err(StorageError::TooLarge.into());
        }
        self.file.persist(&bytes)?;
        self.capture = Some(capture);
        self.fresh = true;
        Ok(())
    }
    pub async fn dispatch(&mut self, service: &impl Service) -> Result<Value, Error> {
        if service.deployment() != self.deployment {
            return Err(Error::Invalid);
        }
        if !self.fresh {
            return Err(Error::Restored);
        }
        let capture = self.capture.as_ref().ok_or(Error::Invalid)?;
        self.fresh = false;
        let lease = self.file.lease_pending()?;
        let result = service.send(capture.kind, capture.arguments.clone()).await;
        lease.check()?;
        drop(lease);
        let result = result?;
        let valid = match capture.kind {
            ChangeKind::CancelRun => result.is_null(),
            _ => result.as_str().is_some_and(|id| {
                !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
            }),
        };
        if !valid {
            return Err(BackendError::UnknownWrite.into());
        }
        self.file
            .append_acknowledgement(br#"{"acknowledged":true}"#)?;
        self.acknowledged = true;
        Ok(result)
    }
    pub fn clear_acknowledged(&mut self) -> Result<(), Error> {
        if !self.acknowledged {
            return Err(Error::Pending);
        }
        self.discard_explicitly()
    }
    /// Explicit local confirmation only. This cannot undo a remote edit.
    pub fn discard_explicitly(&mut self) -> Result<(), Error> {
        self.file.retire()?;
        self.capture = None;
        self.acknowledged = false;
        self.fresh = false;
        Ok(())
    }
}
#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
