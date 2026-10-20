//! Durable permission to start once. Restored requests reconcile but never execute.
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::models::{Claim, Job, Run, RunStatus};
use crate::pending_file::{PendingFile, RecoverySlot, StorageError};

// Two 512000-byte streams can expand sixfold when control bytes are JSON-escaped.
const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub account: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    version: u8,
    pub deployment: String,
    pub request_id: String,
    pub run_id: String,
    pub context: Context,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkerResult {
    pub success: bool,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub duration: f64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredResult {
    version: u8,
    request_id: String,
    run_id: String,
    result: WorkerResult,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "phase", rename_all = "lowercase", deny_unknown_fields)]
enum Marker {
    Started,
    Acknowledged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Storage(StorageError),
    Invalid,
    Pending,
    Restored,
    NotRunning,
}
impl From<StorageError> for Error {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => error.fmt(f),
            Self::Invalid => f.write_str("Automation recovery is unsupported or belongs to another backend. The file is retained."),
            Self::Pending => f.write_str("Resolve the saved automation run before starting another."),
            Self::Restored => f.write_str("This run was restored. Reconcile its status; its command will not run again."),
            Self::NotRunning => f.write_str("The captured automation run is no longer running."),
        }
    }
}
impl std::error::Error for Error {}

/// No Debug: includes captured prompts, commands and provider identity.
pub struct Permit {
    pub request: Request,
    pub run: Run,
    pub job: Job,
}

pub struct Journal {
    request_file: PendingFile,
    result_file: PendingFile,
    deployment: String,
    request: Option<Request>,
    started: bool,
    acknowledged: bool,
    prepared_here: bool,
    started_here: bool,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn checkpoint(bytes: &[u8], deployment: &str) -> Result<(Request, bool, bool), Error> {
    let mut stream = serde_json::Deserializer::from_slice(bytes).into_iter::<Value>();
    let request: Request = serde_json::from_value(
        stream
            .next()
            .ok_or(Error::Invalid)?
            .map_err(|_| Error::Invalid)?,
    )
    .map_err(|_| Error::Invalid)?;
    let identity = uuid::Uuid::parse_str(&request.request_id).map_err(|_| Error::Invalid)?;
    if request.version != 1
        || identity.get_version_num() != 4
        || identity.to_string() != request.request_id
        || request.deployment != deployment
        || !valid_id(&request.run_id)
        || request
            .context
            .account
            .as_deref()
            .is_some_and(|value| !valid_id(value))
        || request
            .context
            .model
            .as_deref()
            .is_some_and(|value| !valid_id(value))
    {
        return Err(Error::Invalid);
    }
    let mut started = false;
    let mut acknowledged = false;
    for value in stream {
        let marker: Marker = serde_json::from_value(value.map_err(|_| Error::Invalid)?)
            .map_err(|_| Error::Invalid)?;
        match marker {
            Marker::Started if !started && !acknowledged => started = true,
            Marker::Acknowledged if !acknowledged => acknowledged = true,
            _ => return Err(Error::Invalid),
        }
    }
    Ok((request, started, acknowledged))
}

impl Journal {
    pub fn open(config: &Path, deployment: &str) -> Result<Self, Error> {
        let deployment = super::backend::AutomationBackend::new(deployment)
            .map_err(|_| Error::Invalid)?
            .deployment()
            .to_owned();
        let request_file = PendingFile::open(config, RecoverySlot::Automation, MAX_BYTES)?;
        let result_file = PendingFile::open(config, RecoverySlot::AutomationResult, MAX_BYTES)?;
        let (request, started, acknowledged) = match request_file.current() {
            Some(bytes) => {
                let (request, started, acknowledged) = checkpoint(bytes, &deployment)?;
                (Some(request), started, acknowledged)
            }
            None if result_file.current().is_some() => return Err(Error::Invalid),
            None => (None, false, false),
        };
        let journal = Self {
            request_file,
            result_file,
            deployment,
            request,
            started,
            acknowledged,
            prepared_here: false,
            started_here: false,
        };
        journal.result()?;
        Ok(journal)
    }

    pub fn request(&self) -> Option<&Request> {
        self.request.as_ref()
    }
    pub fn needs_reconciliation(&self) -> bool {
        self.request.is_some()
    }
    pub fn acknowledged(&self) -> bool {
        self.acknowledged
    }

    pub(super) fn may_start(&self) -> bool {
        self.prepared_here && !self.started && !self.acknowledged
    }

    pub fn prepare(&mut self, run_id: &str, context: Context) -> Result<(), Error> {
        if self.request.is_some() || self.result_file.current().is_some() {
            return Err(Error::Pending);
        }
        if !valid_id(run_id) {
            return Err(Error::Invalid);
        }
        let request = Request {
            version: 1,
            deployment: self.deployment.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
            run_id: run_id.into(),
            context,
        };
        let bytes = serde_json::to_vec(&request).map_err(|_| Error::Invalid)?;
        checkpoint(&bytes, &self.deployment)?;
        self.request_file.persist(&bytes)?;
        self.request = Some(request);
        self.prepared_here = true;
        Ok(())
    }

    /// The durable marker must succeed before the caller receives execution permission.
    pub fn start(&mut self, claim: &Claim) -> Result<Permit, Error> {
        if !self.prepared_here || self.started || self.acknowledged {
            return Err(Error::Restored);
        }
        let request = self.request.as_ref().ok_or(Error::Invalid)?;
        let Claim::Claimed {
            request_id,
            run,
            job,
        } = claim
        else {
            return Err(Error::NotRunning);
        };
        if *request_id != request.request_id
            || run.id != request.run_id
            || run.job_id != job.id
            || run.status != RunStatus::Running
        {
            return Err(Error::NotRunning);
        }
        self.request_file.append_acknowledgement(
            &serde_json::to_vec(&Marker::Started).map_err(|_| Error::Invalid)?,
        )?;
        self.started = true;
        self.started_here = true;
        Ok(Permit {
            request: request.clone(),
            run: *run.clone(),
            job: *job.clone(),
        })
    }

    pub fn record_result(&mut self, result: WorkerResult) -> Result<(), Error> {
        if !self.started_here || self.acknowledged {
            return Err(Error::Restored);
        }
        if !result.duration.is_finite()
            || result.duration < 0.0
            || (result.success && result.error.is_some())
        {
            return Err(Error::Invalid);
        }
        let request = self.request.as_ref().ok_or(Error::Invalid)?;
        let stored = StoredResult {
            version: 1,
            request_id: request.request_id.clone(),
            run_id: request.run_id.clone(),
            result,
        };
        // A second window may have retired this request after we started it.
        // Hold its original file lease until the matching result is durable.
        let lease = self.request_file.lease_pending()?;
        self.result_file
            .persist(&serde_json::to_vec(&stored).map_err(|_| Error::Invalid)?)?;
        lease.check()?;
        Ok(())
    }

    pub fn result(&self) -> Result<Option<WorkerResult>, Error> {
        let Some(bytes) = self.result_file.current() else {
            return Ok(None);
        };
        let request = self.request.as_ref().ok_or(Error::Invalid)?;
        let stored: StoredResult = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        if stored.version != 1
            || stored.request_id != request.request_id
            || stored.run_id != request.run_id
            || !stored.result.duration.is_finite()
            || stored.result.duration < 0.0
            || (stored.result.success && stored.result.error.is_some())
            || !self.started
        {
            return Err(Error::Invalid);
        }
        Ok(Some(stored.result))
    }

    /// Only the original claim's terminal server status permits local retirement.
    pub fn acknowledge_terminal(&mut self, claim: &Claim) -> Result<(), Error> {
        let request = self.request.as_ref().ok_or(Error::Invalid)?;
        let valid = match claim {
            Claim::Retired { request_id, run_id } => {
                *request_id == request.request_id && *run_id == request.run_id
            }
            Claim::Claimed {
                request_id, run, ..
            } => {
                *request_id == request.request_id
                    && run.id == request.run_id
                    && !run.status.is_active()
            }
        };
        if !valid {
            return Err(Error::NotRunning);
        }
        if !self.acknowledged {
            self.request_file.append_acknowledgement(
                &serde_json::to_vec(&Marker::Acknowledged).map_err(|_| Error::Invalid)?,
            )?;
            self.acknowledged = true;
        }
        self.retire_acknowledged()
    }

    pub fn retire_acknowledged(&mut self) -> Result<(), Error> {
        if !self.acknowledged {
            return Err(Error::Pending);
        }
        self.result_file.retire()?;
        self.request_file.retire()?;
        self.request = None;
        self.prepared_here = false;
        self.started_here = false;
        self.started = false;
        self.acknowledged = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claim(request: &Request, status: &str) -> Claim {
        Claim::parse(json!({"kind":"claimed","requestId":request.request_id,
            "run":{"_id":request.run_id,"jobId":"job","status":status,"startedAt":1.0},
            "job":{"_id":"job","name":"fixture","workerType":"exec","config":{"command":"fixture"}}}),
            &request.request_id, &request.run_id).unwrap()
    }

    #[test]
    fn restart_never_issues_a_second_execution_permit_and_keeps_result_bytes() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.json");
        std::fs::write(&config, b"{}").unwrap();
        let mut first = Journal::open(&config, "http://127.0.0.1:3210").unwrap();
        first
            .prepare(
                "run-one",
                Context {
                    account: Some("captured-account".into()),
                    model: Some("captured-model".into()),
                },
            )
            .unwrap();
        let request = first.request().unwrap().clone();
        let running = claim(&request, "running");
        let mut second = Journal::open(&config, "http://127.0.0.1:3210").unwrap();
        assert!(matches!(second.start(&running), Err(Error::Restored)));
        let permit = first.start(&running).unwrap();
        assert_eq!(
            permit.request.context.account.as_deref(),
            Some("captured-account")
        );
        assert!(matches!(first.start(&running), Err(Error::Restored)));
        let result = WorkerResult {
            success: true,
            output: Some(json!({"stdout":"雪\n","stderr":""})),
            error: None,
            duration: 12.0,
        };
        first.record_result(result.clone()).unwrap();
        let saved = std::fs::read(first.result_file.path()).unwrap();
        drop(first);
        let mut restored = Journal::open(&config, "http://127.0.0.1:3210").unwrap();
        assert!(matches!(restored.start(&running), Err(Error::Restored)));
        assert!(restored.result().unwrap() == Some(result));
        assert_eq!(std::fs::read(restored.result_file.path()).unwrap(), saved);
        assert_eq!(
            restored.acknowledge_terminal(&running),
            Err(Error::NotRunning)
        );
        restored
            .acknowledge_terminal(&claim(&request, "completed"))
            .unwrap();
        assert!(!restored.needs_reconciliation());
        assert!(
            !Journal::open(&config, "http://127.0.0.1:3210")
                .unwrap()
                .needs_reconciliation()
        );
    }

    #[test]
    fn changed_or_malformed_storage_blocks_start_and_preserves_original_bytes() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.json");
        std::fs::write(&config, b"{}").unwrap();
        let mut journal = Journal::open(&config, "http://127.0.0.1:3210").unwrap();
        journal.prepare("run-one", Context::default()).unwrap();
        let running = claim(journal.request().unwrap(), "running");
        let mut bytes = std::fs::read(journal.request_file.path()).unwrap();
        bytes.push(b'{');
        std::fs::write(journal.request_file.path(), &bytes).unwrap();
        assert!(matches!(
            journal.start(&running),
            Err(Error::Storage(StorageError::Changed))
        ));
        assert!(matches!(
            Journal::open(&config, "http://127.0.0.1:3210"),
            Err(Error::Invalid)
        ));
        assert_eq!(std::fs::read(journal.request_file.path()).unwrap(), bytes);
    }

    #[test]
    fn restored_prepared_claim_and_a_changed_backend_cannot_start() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.json");
        std::fs::write(&config, b"{}").unwrap();
        let mut journal = Journal::open(&config, "http://127.0.0.1:3210").unwrap();
        journal.prepare("run-one", Context::default()).unwrap();
        let request = journal.request().unwrap().clone();
        let before = std::fs::read(journal.request_file.path()).unwrap();
        let mut restored = Journal::open(&config, "http://127.0.0.1:3210").unwrap();
        assert!(matches!(
            restored.start(&claim(&request, "running")),
            Err(Error::Restored)
        ));
        assert!(matches!(
            Journal::open(&config, "http://127.0.0.1:3211"),
            Err(Error::Invalid)
        ));
        assert_eq!(std::fs::read(journal.request_file.path()).unwrap(), before);
        restored
            .acknowledge_terminal(&Claim::Retired {
                request_id: request.request_id,
                run_id: request.run_id,
            })
            .unwrap();
    }

    #[test]
    fn bounded_streams_with_json_escape_expansion_survive_recovery() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.json");
        std::fs::write(&config, b"{}").unwrap();
        let mut journal = Journal::open(&config, "http://127.0.0.1:3210").unwrap();
        journal.prepare("run-one", Context::default()).unwrap();
        journal
            .start(&claim(journal.request().unwrap(), "running"))
            .unwrap();
        let output = json!({"stdout":"\0".repeat(512000),"stderr":"\0".repeat(512000)});
        journal
            .record_result(WorkerResult {
                success: false,
                output: Some(output.clone()),
                error: Some("fixture".into()),
                duration: 1.0,
            })
            .unwrap();
        let restored = Journal::open(&config, "http://127.0.0.1:3210").unwrap();
        assert_eq!(restored.result().unwrap().unwrap().output, Some(output));
    }

    #[test]
    fn an_old_window_cannot_write_an_orphan_result_after_another_window_retires_its_run() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.json");
        std::fs::write(&config, b"{}").unwrap();
        let mut first = Journal::open(&config, "http://127.0.0.1:3210").unwrap();
        first.prepare("run-one", Context::default()).unwrap();
        let request = first.request().unwrap().clone();
        first.start(&claim(&request, "running")).unwrap();
        let mut second = Journal::open(&config, "http://127.0.0.1:3210").unwrap();
        second
            .acknowledge_terminal(&claim(&request, "cancelled"))
            .unwrap();
        assert_eq!(
            first.record_result(WorkerResult {
                success: false,
                output: Some(json!("late output")),
                error: Some("cancelled".into()),
                duration: 1.0
            }),
            Err(Error::Storage(StorageError::Changed))
        );
        assert!(
            !Journal::open(&config, "http://127.0.0.1:3210")
                .unwrap()
                .needs_reconciliation()
        );
    }
}
