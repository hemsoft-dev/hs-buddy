//! Assistant persistence uses static errors and request-bound acknowledgements.
use serde::Deserialize;
use serde_json::{Value, json};

use crate::copilot_results::summaries::{Page, ResultSummary, ThreadStats};
use crate::copilot_runs::{Execution, Failure, Phase, ReviewContext};
use crate::terminal_backend::{BackendError, TerminalBackend};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    UnknownWrite,
    Rejected,
    InvalidResponse,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "Assistant history is unavailable. Try refreshing.",
            Self::UnknownWrite => {
                "The assistant save is unconfirmed. Reconcile this request before continuing."
            }
            Self::Rejected => {
                "The backend rejected this assistant change. Existing output is retained."
            }
            Self::InvalidResponse => "The assistant backend returned an unsupported response.",
        })
    }
}
impl std::error::Error for Error {}

impl From<BackendError> for Error {
    fn from(error: BackendError) -> Self {
        match error {
            BackendError::Unavailable => Self::Unavailable,
            BackendError::UnknownWrite => Self::UnknownWrite,
            BackendError::Rejected => Self::Rejected,
            _ => Self::InvalidResponse,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Deleted,
}

#[derive(Clone, Deserialize)]
pub struct BeginReceipt {
    pub id: String,
    pub created: bool,
    pub status: RecordStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct ClaimReceipt {
    pub claimed: bool,
    pub status: RecordStatus,
}

/// No Debug: the title and request payload may contain private text.
#[derive(Clone)]
pub struct ReviewSnapshot {
    pub account: String,
    pub context: ReviewContext,
    pub title: String,
    pub threads: ThreadStats,
}

/// Immutable payload and UUID survive reconciliation and storage-only retries.
pub struct NativeRequest {
    identity: String,
    arguments: Value,
}

impl NativeRequest {
    pub fn new(execution: &Execution, review: Option<&ReviewSnapshot>) -> Result<Self, Failure> {
        if execution.phase() != Phase::AwaitingRecord
            || execution.context().review.as_ref() != review.map(|snapshot| &snapshot.context)
            || review.is_some_and(|snapshot| {
                !snapshot
                    .account
                    .eq_ignore_ascii_case(&execution.context().account)
            })
        {
            return Err(Failure::InvalidContext);
        }
        let identity = uuid::Uuid::new_v4().to_string();
        let context = execution.context();
        let mut arguments = json!({
            "requestId": identity, "prompt": execution.prompt(),
            "model": context.model, "account": context.account,
        });
        if let Some(snapshot) = review {
            let (owner, repo) = snapshot
                .context
                .repository
                .split_once('/')
                .ok_or(Failure::InvalidContext)?;
            arguments["review"] = json!({
                "owner": owner, "repo": repo, "prNumber": snapshot.context.pull_request,
                "title": snapshot.title, "head": snapshot.context.head,
                "threadStats": {"total":snapshot.threads.total, "unresolved":snapshot.threads.unresolved, "outdated":snapshot.threads.outdated},
            });
        }
        Ok(Self {
            identity,
            arguments,
        })
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}

fn begin_receipt(value: Value) -> Result<BeginReceipt, Error> {
    let receipt: BeginReceipt =
        serde_json::from_value(value).map_err(|_| Error::InvalidResponse)?;
    if !valid_id(&receipt.id) || (receipt.created && receipt.status != RecordStatus::Pending) {
        return Err(Error::InvalidResponse);
    }
    Ok(receipt)
}

fn claim_receipt(value: Value) -> Result<ClaimReceipt, Error> {
    let receipt: ClaimReceipt = serde_json::from_value(value).map_err(|_| Error::UnknownWrite)?;
    if receipt.claimed && receipt.status != RecordStatus::Running {
        return Err(Error::UnknownWrite);
    }
    Ok(receipt)
}

fn finish_receipt(value: &Value, id: &str, failed: bool) -> Result<(), Error> {
    let expected = if failed { "failed" } else { "completed" };
    if value["id"].as_str() != Some(id)
        || value["saved"].as_bool() != Some(true)
        || value["status"].as_str() != Some(expected)
    {
        return Err(Error::UnknownWrite);
    }
    Ok(())
}

pub fn failure_code(failure: Failure) -> &'static str {
    match failure {
        Failure::AuthenticationRequired => "authentication-required",
        Failure::SendUnknown | Failure::RecordUnknown => "delivery-unknown",
        Failure::IncompleteStream => "incomplete-stream",
        Failure::OutputLimit | Failure::EventLimit => "output-limit",
        _ => "service-unavailable",
    }
}

#[derive(Clone)]
pub struct CopilotBackend {
    client: TerminalBackend,
}

#[derive(Clone, PartialEq, Eq)]
pub enum HistoryFilter {
    All,
    Status(crate::copilot_results::Status),
    Category(String),
}

fn history_request(
    filter: &HistoryFilter,
    cursor: Option<&str>,
    num_items: u8,
) -> Result<(&'static str, Value), Error> {
    if !(1..=25).contains(&num_items) || cursor.is_some_and(|cursor| cursor.len() > 16 * 1024) {
        return Err(Error::Rejected);
    }
    let mut args = json!({"paginationOpts":{"numItems":num_items,"cursor":cursor}});
    let function = match filter {
        HistoryFilter::All => "copilotNative:listResults",
        HistoryFilter::Status(status) => {
            use crate::copilot_results::Status;
            let status = match status {
                Status::Pending => "pending",
                Status::Running => "running",
                Status::Completed => "completed",
                Status::Failed => "failed",
                Status::Unknown => return Err(Error::Rejected),
            };
            args["status"] = json!(status);
            "copilotNative:listResultsByStatus"
        }
        HistoryFilter::Category(category) => {
            if !valid_id(category) {
                return Err(Error::Rejected);
            }
            args["category"] = json!(category);
            "copilotNative:listResultsByCategory"
        }
    };
    Ok((function, args))
}

impl CopilotBackend {
    pub(crate) fn deployment(&self) -> &str {
        self.client.deployment()
    }
    pub fn new(url: &str) -> Result<Self, Error> {
        Ok(Self {
            client: TerminalBackend::new(url)?,
        })
    }

    pub async fn begin(&self, request: &NativeRequest) -> Result<BeginReceipt, Error> {
        begin_receipt(
            self.client
                .call("copilotNativeWrites:begin", request.arguments.clone(), true)
                .await?,
        )
    }

    pub async fn claim(&self, request: &NativeRequest, id: &str) -> Result<ClaimReceipt, Error> {
        if !valid_id(id) {
            return Err(Error::Rejected);
        }
        claim_receipt(
            self.client
                .call(
                    "copilotNativeWrites:claim",
                    json!({"id": id, "requestId": request.identity}),
                    true,
                )
                .await?,
        )
    }

    pub async fn reconcile(
        &self,
        request: &NativeRequest,
    ) -> Result<Option<(String, RecordStatus)>, Error> {
        let value = self
            .client
            .call(
                "copilotNativeWrites:reconcile",
                json!({"requestId":request.identity}),
                false,
            )
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        #[derive(Deserialize)]
        struct Receipt {
            id: String,
            status: RecordStatus,
        }
        let receipt: Receipt = serde_json::from_value(value).map_err(|_| Error::InvalidResponse)?;
        if !valid_id(&receipt.id) {
            return Err(Error::InvalidResponse);
        }
        Ok(Some((receipt.id, receipt.status)))
    }

    pub async fn finish(
        &self,
        request: &NativeRequest,
        id: &str,
        output: &str,
        failure: Option<&'static str>,
    ) -> Result<(), Error> {
        if !valid_id(id)
            || request.arguments["prompt"]
                .as_str()
                .map_or(usize::MAX, str::len)
                .saturating_add(output.len())
                > 768 * 1024
        {
            return Err(Error::Rejected);
        }
        let mut arguments = json!({"id":id,"requestId":request.identity,"result":output});
        if let Some(code) = failure {
            arguments["failure"] = json!(code);
        }
        let value = self
            .client
            .call("copilotNativeWrites:finish", arguments, true)
            .await?;
        finish_receipt(&value, id, failure.is_some())
    }

    pub async fn history(
        &self,
        cursor: Option<&str>,
        num_items: u8,
        filter: &HistoryFilter,
    ) -> Result<Page<ResultSummary>, Error> {
        let (function, args) = history_request(filter, cursor, num_items)?;
        let value = self.client.call(function, args, false).await?;
        let bytes = serde_json::to_vec(&value).map_err(|_| Error::InvalidResponse)?;
        crate::copilot_results::summaries::parse_results(&bytes).map_err(|_| Error::InvalidResponse)
    }

    pub async fn detail(
        &self,
        id: &str,
    ) -> Result<Option<crate::copilot_results::SavedResult>, Error> {
        if !valid_id(id) {
            return Err(Error::Rejected);
        }
        let value = self
            .client
            .call("copilotResults:get", json!({"id":id}), false)
            .await?;
        let bytes = serde_json::to_vec(&value).map_err(|_| Error::InvalidResponse)?;
        crate::copilot_results::parse_detail(&bytes).map_err(|_| Error::InvalidResponse)
    }

    pub async fn remove(&self, id: &str) -> Result<(), Error> {
        if !valid_id(id) {
            return Err(Error::Rejected);
        }
        let value = self
            .client
            .call("copilotResults:remove", json!({"id":id}), true)
            .await?;
        if !value.is_null() {
            return Err(Error::UnknownWrite);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
