//! Manual queue creation captures its own idempotence identity before any send.
use super::backend::{AutomationBackend, Error as BackendError};
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
    pub request_id: String,
    pub job_id: String,
    #[serde(
        default,
        deserialize_with = "super::models::present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub input: Option<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Recorded,
    Retired,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub kind: Outcome,
    pub request_id: String,
    pub run_id: String,
}

#[async_trait]
pub trait Service: Send + Sync {
    fn deployment(&self) -> &str;
    async fn send(&self, capture: &Capture) -> Result<Receipt, BackendError>;
    async fn lookup(&self, request_id: &str) -> Result<Option<Receipt>, BackendError>;
}

#[async_trait]
impl Service for AutomationBackend {
    fn deployment(&self) -> &str {
        self.deployment()
    }
    async fn send(&self, capture: &Capture) -> Result<Receipt, BackendError> {
        self.trigger_manual(&capture.request_id, &capture.job_id, capture.input.clone())
            .await
    }
    async fn lookup(&self, request_id: &str) -> Result<Option<Receipt>, BackendError> {
        self.lookup_manual_trigger(request_id).await
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Storage(StorageError),
    Backend(BackendError),
    Invalid,
    Pending,
    Restored,
}
impl From<StorageError> for Error {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}
impl From<BackendError> for Error {
    fn from(error: BackendError) -> Self {
        Self::Backend(error)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => error.fmt(f),
            Self::Backend(error) => error.fmt(f),
            Self::Invalid => f.write_str(
                "The saved automation request does not match this backend. Its file is retained.",
            ),
            Self::Pending => f.write_str(
                "Resolve the captured automation request before starting another change.",
            ),
            Self::Restored => {
                f.write_str("Check the saved request first. It will not be sent automatically.")
            }
        }
    }
}
impl std::error::Error for Error {}

pub struct Trigger {
    file: PendingFile,
    deployment: String,
    capture: Option<Capture>,
    receipt: Option<Receipt>,
    fresh: bool,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

impl Trigger {
    pub fn open(config: &Path, deployment: &str) -> Result<Self, Error> {
        let deployment = AutomationBackend::new(deployment)
            .map_err(|_| Error::Invalid)?
            .deployment()
            .to_owned();
        let file = PendingFile::open(config, RecoverySlot::AutomationTrigger, MAX_BYTES)?;
        let mut state = Self {
            file,
            deployment,
            capture: None,
            receipt: None,
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
            let id = uuid::Uuid::parse_str(&capture.request_id).map_err(|_| Error::Invalid)?;
            if capture.version != 1
                || id.get_version_num() != 4
                || id.to_string() != capture.request_id
                || capture.deployment != state.deployment
                || !valid_id(&capture.job_id)
            {
                return Err(Error::Invalid);
            }
            if let Some(value) = stream.next() {
                let receipt: Receipt = serde_json::from_value(value.map_err(|_| Error::Invalid)?)
                    .map_err(|_| Error::Invalid)?;
                Self::verify(&capture, &receipt)?;
                state.receipt = Some(receipt);
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
    pub fn receipt(&self) -> Option<&Receipt> {
        self.receipt.as_ref()
    }
    pub fn prepare(&mut self, job_id: &str, input: Option<Value>) -> Result<(), Error> {
        if self.capture.is_some() {
            return Err(Error::Pending);
        }
        if !valid_id(job_id) {
            return Err(Error::Invalid);
        }
        let capture = Capture {
            version: 1,
            deployment: self.deployment.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
            job_id: job_id.into(),
            input,
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
    fn verify(capture: &Capture, receipt: &Receipt) -> Result<(), Error> {
        if receipt.request_id != capture.request_id || !valid_id(&receipt.run_id) {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    fn acknowledge(&mut self, receipt: Receipt) -> Result<Receipt, Error> {
        Self::verify(self.capture.as_ref().ok_or(Error::Invalid)?, &receipt)?;
        if let Some(original) = &self.receipt {
            if original != &receipt {
                return Err(Error::Invalid);
            }
        } else {
            self.file.append_acknowledgement(
                &serde_json::to_vec(&receipt).map_err(|_| Error::Invalid)?,
            )?;
            self.receipt = Some(receipt.clone());
        }
        self.fresh = false;
        Ok(receipt)
    }
    pub async fn dispatch(&mut self, service: &impl Service) -> Result<Receipt, Error> {
        if !self.fresh {
            return Err(Error::Restored);
        }
        self.retry_captured(service).await
    }
    /// User action only. A retry retains the identical UUID, job and input.
    pub async fn retry_captured(&mut self, service: &impl Service) -> Result<Receipt, Error> {
        if service.deployment() != self.deployment || self.receipt.is_some() {
            return Err(Error::Invalid);
        }
        let capture = self.capture.as_ref().ok_or(Error::Invalid)?;
        self.fresh = false;
        let lease = self.file.lease_pending()?;
        let result = service.send(capture).await;
        lease.check()?;
        drop(lease);
        self.acknowledge(result?)
    }
    pub async fn reconcile(&mut self, service: &impl Service) -> Result<Option<Receipt>, Error> {
        if service.deployment() != self.deployment {
            return Err(Error::Invalid);
        }
        if let Some(receipt) = &self.receipt {
            return Ok(Some(receipt.clone()));
        }
        let capture = self.capture.as_ref().ok_or(Error::Invalid)?;
        let lease = self.file.lease_pending()?;
        let result = service.lookup(&capture.request_id).await;
        lease.check()?;
        drop(lease);
        result?.map(|receipt| self.acknowledge(receipt)).transpose()
    }
    pub fn clear_acknowledged(&mut self) -> Result<(), Error> {
        if self.receipt.is_none() {
            return Err(Error::Pending);
        }
        self.discard_explicitly()
    }
    /// Explicit user action for a request the server rejected or never confirmed.
    /// A lost request may still have queued its run; this only removes the local copy.
    pub fn discard_explicitly(&mut self) -> Result<(), Error> {
        self.file.retire()?;
        self.capture = None;
        self.receipt = None;
        self.fresh = false;
        Ok(())
    }
}

#[cfg(test)]
#[path = "trigger_tests.rs"]
mod tests;
