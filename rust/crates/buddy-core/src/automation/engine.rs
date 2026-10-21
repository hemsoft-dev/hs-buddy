//! One local queue coordinator. Network retries reconcile captured requests;
//! only a durable fresh permit reaches the worker factory.
use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;

use super::backend::{AutomationBackend, ChangeKind, Error as BackendError};
use super::journal::{Context, Error as JournalError, Journal, Permit, Request, WorkerResult};
use super::models::{Claim, Run, RunStatus};

#[async_trait]
pub trait Queue: Send + Sync {
    fn deployment(&self) -> &str;
    async fn recover_missed(&self) -> Result<(), BackendError>;
    async fn pending(&self) -> Result<Option<Run>, BackendError>;
    async fn lookup(&self, request: &Request) -> Result<Option<Claim>, BackendError>;
    async fn claim(&self, request: &Request) -> Result<Claim, BackendError>;
    async fn run(&self, id: &str) -> Result<Option<Run>, BackendError>;
    async fn finish(&self, request: &Request, result: &WorkerResult) -> Result<(), BackendError>;
    async fn cancel(&self, id: &str) -> Result<(), BackendError>;
}

/// One schedule the server rejects (say, a legacy invalid cron) must not stop
/// every other queued run; it stays visible in the Schedules tab. Unavailable or
/// unconfirmed requests still stop the pass so it is retried.
async fn recover_each<F, R>(
    ids: impl IntoIterator<Item = String>,
    mut recover: F,
) -> Result<(), BackendError>
where
    F: FnMut(String) -> R,
    R: std::future::Future<Output = Result<(), BackendError>>,
{
    for id in ids {
        match recover(id).await {
            Ok(()) | Err(BackendError::Rejected) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[async_trait]
impl Queue for AutomationBackend {
    fn deployment(&self) -> &str {
        self.deployment()
    }
    async fn recover_missed(&self) -> Result<(), BackendError> {
        let enabled = self
            .schedules()
            .await?
            .into_iter()
            .filter(|schedule| schedule.enabled)
            .map(|schedule| schedule.id);
        recover_each(enabled, |id| async move { self.recover_missed(&id).await }).await
    }
    async fn pending(&self) -> Result<Option<Run>, BackendError> {
        let mut runs = self.pending().await?;
        if runs.len() > 1 || runs.iter().any(|run| run.status != RunStatus::Pending) {
            return Err(BackendError::InvalidResponse);
        }
        Ok(runs.pop())
    }
    async fn lookup(&self, request: &Request) -> Result<Option<Claim>, BackendError> {
        self.reconcile(&request.request_id, &request.run_id).await
    }
    async fn claim(&self, request: &Request) -> Result<Claim, BackendError> {
        self.claim(&request.request_id, &request.run_id).await
    }
    async fn run(&self, id: &str) -> Result<Option<Run>, BackendError> {
        self.run(id).await
    }
    async fn finish(&self, request: &Request, result: &WorkerResult) -> Result<(), BackendError> {
        let output = result.output.clone().unwrap_or(Value::Null);
        if result.success {
            self.complete(&request.request_id, &request.run_id, output)
                .await
        } else {
            self.fail(
                &request.request_id,
                &request.run_id,
                result.error.as_deref().unwrap_or("The worker failed"),
                output,
            )
            .await
        }
    }
    async fn cancel(&self, id: &str) -> Result<(), BackendError> {
        self.change(ChangeKind::CancelRun, serde_json::json!({"id":id}))
            .await
            .map(|_| ())
    }
}

pub trait Worker: Send {
    fn result(&self) -> Option<WorkerResult>;
    fn output(&self) -> (String, String);
    fn cancel(&self);
}

impl Worker for super::exec::Session {
    fn result(&self) -> Option<WorkerResult> {
        self.result()
    }
    fn output(&self) -> (String, String) {
        self.output()
    }
    fn cancel(&self) {
        self.cancel();
    }
}

pub trait Workers: Send {
    fn start(&mut self, permit: Permit) -> Result<Box<dyn Worker>, WorkerResult>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Claiming,
    Running,
    Reconciling,
    Attention,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Backend(BackendError),
    Journal(JournalError),
}
impl From<BackendError> for Error {
    fn from(value: BackendError) -> Self {
        Self::Backend(value)
    }
}
impl From<JournalError> for Error {
    fn from(value: JournalError) -> Self {
        Self::Journal(value)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(error) => error.fmt(f),
            Self::Journal(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for Error {}

pub struct Engine<Q: Queue, W: Workers> {
    queue: Q,
    workers: W,
    journal: Journal,
    active: Option<Box<dyn Worker>>,
    recovered: bool,
    phase: Phase,
}

impl<Q: Queue, W: Workers> Engine<Q, W> {
    pub fn open(queue: Q, workers: W, config: &Path) -> Result<Self, Error> {
        let journal = Journal::open(config, queue.deployment())?;
        let phase = if journal.needs_reconciliation() {
            Phase::Reconciling
        } else {
            Phase::Idle
        };
        Ok(Self {
            queue,
            workers,
            journal,
            active: None,
            recovered: false,
            phase,
        })
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn request(&self) -> Option<&Request> {
        self.journal.request()
    }
    pub fn output(&self) -> Option<(String, String)> {
        self.active.as_ref().map(|worker| worker.output())
    }

    /// A background caller owns cadence. There is no local cron scanner.
    pub async fn tick(&mut self, context: Context) -> Result<Phase, Error> {
        let result = self.advance(context).await;
        if matches!(result, Err(Error::Backend(BackendError::Unavailable))) {
            self.recovered = false;
        }
        result
    }

    async fn advance(&mut self, context: Context) -> Result<Phase, Error> {
        // No worker borrow is held across an await: workers need not be Sync.
        if let Some(finished) = self.active.as_ref().map(|worker| worker.result()) {
            if let Some(result) = finished {
                // Keep the live handle/result on disk failure; never run it again.
                self.journal.record_result(result)?;
                self.active.take();
            } else {
                self.phase = Phase::Running;
                let run_id = self
                    .journal
                    .request()
                    .ok_or(JournalError::Invalid)?
                    .run_id
                    .clone();
                if self
                    .queue
                    .run(&run_id)
                    .await?
                    .is_none_or(|run| !run.status.is_active())
                    && let Some(worker) = &self.active
                {
                    worker.cancel();
                }
                return Ok(self.phase);
            }
        }
        if self.journal.acknowledged() {
            self.journal.retire_acknowledged()?;
            self.phase = Phase::Idle;
            return Ok(self.phase);
        }
        if self.journal.request().is_some() {
            return self.settle().await;
        }
        if !self.recovered {
            self.queue.recover_missed().await?;
            self.recovered = true;
        }
        let Some(run) = self.queue.pending().await? else {
            self.phase = Phase::Idle;
            return Ok(self.phase);
        };
        if run.status != RunStatus::Pending {
            return Err(BackendError::InvalidResponse.into());
        }
        self.journal.prepare(&run.id, context)?;
        self.settle().await
    }

    async fn settle(&mut self) -> Result<Phase, Error> {
        let request = self.journal.request().ok_or(JournalError::Invalid)?.clone();
        self.phase = Phase::Reconciling;
        let claim = match self.queue.lookup(&request).await? {
            Some(claim) => claim,
            None if self.journal.may_start() => {
                self.phase = Phase::Claiming;
                self.queue.claim(&request).await?
            }
            // An old unconfirmed request is never permission to take new work.
            None => {
                self.phase = Phase::Attention;
                return Ok(self.phase);
            }
        };
        let result = self.journal.result()?;
        if let Claim::Claimed { run, .. } = &claim {
            if run.status == RunStatus::Cancelled
                && run.output.is_none()
                && let Some(result) = &result
            {
                self.queue.finish(&request, result).await?;
                return Ok(self.phase);
            }
            if run.status.is_active() {
                if let Some(result) = result {
                    self.queue.finish(&request, &result).await?;
                    return Ok(self.phase);
                }
                if !self.journal.may_start() {
                    self.phase = Phase::Attention;
                    return Ok(self.phase);
                }
                let permit = self.journal.start(&claim)?;
                match self.workers.start(permit) {
                    Ok(worker) => {
                        self.active = Some(worker);
                        self.phase = Phase::Running;
                    }
                    Err(result) => {
                        self.journal.record_result(result)?;
                        self.phase = Phase::Reconciling;
                    }
                }
                return Ok(self.phase);
            }
        }
        self.journal.acknowledge_terminal(&claim)?;
        self.phase = Phase::Idle;
        Ok(self.phase)
    }

    pub async fn cancel(&mut self) -> Result<(), Error> {
        let request = self.journal.request().ok_or(JournalError::Invalid)?.clone();
        let cancelled = self.queue.cancel(&request.run_id).await;
        if let Some(worker) = &self.active {
            // Record the cancellation on the server before stopping the worker. A
            // worker stopped while the server still shows it running would later be
            // reported as an ordinary failure. Rejected means the run already ended.
            if let Err(error) = cancelled
                && !matches!(error, BackendError::Rejected)
            {
                return Err(error.into());
            }
            worker.cancel();
        } else {
            // A candidate that already finished elsewhere rejects cancellation;
            // its idempotent claim still retires this request.
            if let Err(error) = cancelled
                && !matches!(error, BackendError::Rejected)
            {
                return Err(error.into());
            }
            if self.journal.result()?.is_some() {
                // A finished worker's saved output is reported first; the next
                // tick publishes it to the cancelled run, then retires the request.
                self.phase = Phase::Reconciling;
                return Ok(());
            }
            let claim = self.queue.claim(&request).await?;
            self.journal.acknowledge_terminal(&claim)?;
            self.phase = Phase::Idle;
        }
        Ok(())
    }
}

impl<Q: Queue, W: Workers> Drop for Engine<Q, W> {
    fn drop(&mut self) {
        if let Some(worker) = &self.active {
            worker.cancel();
        }
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
