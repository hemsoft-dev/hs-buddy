//! Reuse the existing Convex transport. Unknown writes never retry themselves.
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::models::{Claim, Job, Run, Schedule};
use crate::terminal_backend::{BackendError, TerminalBackend};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    UnknownWrite,
    Rejected,
    InvalidResponse,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "Automation is offline. Local work and drafts are retained.",
            Self::UnknownWrite => "This automation change is unconfirmed. Reconcile it before sending another request.",
            Self::Rejected => "The backend rejected this automation change. Your draft is retained.",
            Self::InvalidResponse => "Automation returned unsupported data. Existing work is retained.",
        })
    }
}
impl std::error::Error for Error {}
impl From<BackendError> for Error {
    fn from(value: BackendError) -> Self {
        match value {
            BackendError::Unavailable => Self::Unavailable,
            BackendError::UnknownWrite => Self::UnknownWrite,
            BackendError::Rejected => Self::Rejected,
            _ => Self::InvalidResponse,
        }
    }
}

#[derive(Clone)]
pub struct AutomationBackend {
    client: TerminalBackend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    CreateJob,
    UpdateJob,
    DeleteJob,
    CreateSchedule,
    UpdateSchedule,
    DeleteSchedule,
    CancelRun,
}

impl ChangeKind {
    fn function(self) -> &'static str {
        match self {
            Self::CreateJob => "jobs:create",
            Self::UpdateJob => "jobs:update",
            Self::DeleteJob => "jobs:remove",
            Self::CreateSchedule => "schedules:create",
            Self::UpdateSchedule => "schedules:update",
            Self::DeleteSchedule => "schedules:remove",
            Self::CancelRun => "runs:cancel",
        }
    }
}

fn parse<T: DeserializeOwned>(value: Value) -> Result<T, Error> {
    serde_json::from_value(value).map_err(|_| Error::InvalidResponse)
}

impl AutomationBackend {
    pub fn new(url: &str) -> Result<Self, Error> {
        Ok(Self {
            client: TerminalBackend::new(url)?,
        })
    }

    pub fn deployment(&self) -> &str {
        self.client.deployment()
    }

    pub async fn jobs(&self) -> Result<Vec<Job>, Error> {
        parse(self.client.call("jobs:list", json!({}), false).await?)
    }

    pub async fn schedules(&self) -> Result<Vec<Schedule>, Error> {
        parse(self.client.call("schedules:list", json!({}), false).await?)
    }

    pub async fn recent_runs(&self) -> Result<Vec<Run>, Error> {
        parse(
            self.client
                .call("runs:listRecent", json!({"limit":100}), false)
                .await?,
        )
    }

    pub async fn pending(&self) -> Result<Vec<Run>, Error> {
        parse(
            self.client
                .call(
                    "runs:listByStatus",
                    json!({"status":"pending","limit":1,"oldestFirst":true}),
                    false,
                )
                .await?,
        )
    }

    pub async fn run(&self, id: &str) -> Result<Option<Run>, Error> {
        parse(
            self.client
                .call("runs:get", json!({"id":id}), false)
                .await?,
        )
    }

    pub async fn recover_missed(&self, schedule_id: &str) -> Result<(), Error> {
        self.client
            .call("schedules:recoverMissed", json!({"id":schedule_id}), true)
            .await?;
        Ok(())
    }

    pub async fn change(&self, kind: ChangeKind, arguments: Value) -> Result<Value, Error> {
        self.client
            .call(kind.function(), arguments, true)
            .await
            .map_err(Into::into)
    }

    pub(super) async fn trigger_manual(
        &self,
        request_id: &str,
        job_id: &str,
        input: Option<Value>,
    ) -> Result<super::trigger::Receipt, Error> {
        let mut args = json!({"requestId": request_id, "jobId": job_id});
        if let Some(input) = input {
            args["input"] = input;
        }
        let value = self.client.call("runs:triggerNative", args, true).await?;
        parse(value).map_err(|_| Error::UnknownWrite)
    }

    pub(super) async fn lookup_manual_trigger(
        &self,
        request_id: &str,
    ) -> Result<Option<super::trigger::Receipt>, Error> {
        parse(
            self.client
                .call(
                    "runs:getNativeTrigger",
                    json!({"requestId":request_id}),
                    false,
                )
                .await?,
        )
    }

    pub async fn claim(&self, request_id: &str, candidate: &str) -> Result<Claim, Error> {
        let value = self
            .client
            .call(
                "runs:claimNative",
                json!({"requestId":request_id,"runId":candidate}),
                true,
            )
            .await?;
        Claim::parse(value, request_id, candidate).map_err(|_| Error::UnknownWrite)
    }

    pub async fn reconcile(
        &self,
        request_id: &str,
        candidate: &str,
    ) -> Result<Option<Claim>, Error> {
        let value = self
            .client
            .call(
                "runs:getNativeClaim",
                json!({"requestId":request_id}),
                false,
            )
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        Claim::parse(value, request_id, candidate)
            .map(Some)
            .map_err(|_| Error::InvalidResponse)
    }

    pub async fn complete(
        &self,
        request_id: &str,
        run_id: &str,
        output: Value,
    ) -> Result<(), Error> {
        let value = self
            .client
            .call(
                "runs:complete",
                json!({"id":run_id,"claimRequestId":request_id,"output":output}),
                true,
            )
            .await?;
        if !value.is_null() {
            return Err(Error::UnknownWrite);
        }
        Ok(())
    }

    pub async fn fail(
        &self,
        request_id: &str,
        run_id: &str,
        error: &str,
        output: Value,
    ) -> Result<(), Error> {
        let value = self
            .client
            .call(
                "runs:fail",
                json!({"id":run_id,"claimRequestId":request_id,"error":error,"output":output}),
                true,
            )
            .await?;
        if !value.is_null() {
            return Err(Error::UnknownWrite);
        }
        Ok(())
    }
}
