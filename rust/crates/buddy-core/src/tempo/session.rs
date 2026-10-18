//! Move the complete session into one worker. No restored operation can send.
#[path = "session_copy.rs"]
mod copy;
pub use copy::{CopyPlan, CopyReport, copy_start_time};
#[path = "session_templates.rs"]
mod templates;
pub use templates::Templates;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::{
    ChangeError, Client, DateRange, DispatchReport, Draft, Error, JiraClient, Planner,
    PlannerError, SavedChange, SavedRecovery, Snapshot, WritePhase,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    Read(PlannerError),
    Change(ChangeError),
    Domain(Error),
}
impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => error.fmt(f),
            Self::Change(error) => error.fmt(f),
            Self::Domain(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for SessionError {}
impl From<PlannerError> for SessionError {
    fn from(value: PlannerError) -> Self {
        Self::Read(value)
    }
}
impl From<ChangeError> for SessionError {
    fn from(value: ChangeError) -> Self {
        Self::Change(value)
    }
}
impl From<Error> for SessionError {
    fn from(value: Error) -> Self {
        Self::Domain(value)
    }
}

/// The clients, profile, snapshot and journal stay bound to this connection.
/// The caller constructs both clients from one captured credential record.
/// There is no background refresh or automatic write retry.
pub struct Session {
    config: PathBuf,
    client: Arc<Client>,
    jira: Arc<JiraClient>,
    planner: Planner,
    prepared: Option<SavedChange>,
    restored: Option<SavedRecovery>,
    recovery_problem: Option<ChangeError>,
    fresh: bool,
}

/// Complete billing choices belong to this resolved issue's captured project.
/// A failed classification stays unknown and cannot become a false CapEx flag.
pub struct EditorIssue {
    pub issue: super::JiraIssue,
    pub billing: super::ProjectBillingAccounts,
    pub capitalization: Result<Option<bool>, super::JiraError>,
}
pub struct IssueSummary {
    pub issue: super::JiraIssue,
    pub capitalization: Result<Option<bool>, super::JiraError>,
}
impl Session {
    pub fn tempo_origin(&self) -> super::ApiOrigin {
        self.client.api_origin()
    }
    /// The authenticated Jira site stays bound to this session's clients.
    pub fn jira_origin(&self) -> &str {
        self.jira.origin()
    }

    /// This read model never requests billing data or changes a worklog.
    pub async fn issue_summary(
        &self,
        id: super::IssueId,
    ) -> Result<IssueSummary, super::JiraError> {
        let issue = self.jira.issue(&id.get().to_string()).await?;
        let capitalization = self.jira.capitalization(&issue.key).await;
        Ok(IssueSummary {
            issue,
            capitalization,
        })
    }
    pub async fn editor_issue(&self, key: &str) -> Result<EditorIssue, SessionError> {
        let issue = self.jira.issue(key).await.map_err(PlannerError::from)?;
        let project_key = issue
            .key
            .split_once('-')
            .map(|value| value.0)
            .ok_or(super::JiraError::InvalidIssue)
            .map_err(PlannerError::from)?;
        let project = self
            .jira
            .project(project_key)
            .await
            .map_err(PlannerError::from)?;
        let billing = self
            .client
            .project_billing_accounts(project.id)
            .await
            .map_err(PlannerError::from)?;
        let capitalization = self.jira.capitalization(&issue.key).await;
        Ok(EditorIssue {
            issue,
            billing,
            capitalization,
        })
    }
    pub async fn worklog_for_edit(
        &self,
        id: super::WorklogId,
    ) -> Result<super::Worklog, SessionError> {
        self.client
            .worklog_for_edit(&self.snapshot().account, id)
            .await
            .map_err(PlannerError::from)
            .map_err(Into::into)
    }
    pub async fn connect_current_month_at(
        config: &Path,
        client: Arc<Client>,
        jira: Arc<JiraClient>,
        now: DateTime<Utc>,
    ) -> Result<Self, SessionError> {
        let recovered = SavedRecovery::open_in_region(config, &jira, client.api_origin()).await;
        let planner =
            Planner::connect_current_month_at(config, client.clone(), jira.clone(), now).await?;
        let (restored, recovery_problem) = match recovered {
            Ok(Some(value)) if value.draft().author() != &planner.snapshot().account => {
                (None, Some(ChangeError::Domain(Error::WrongConnection)))
            }
            Ok(value) => (value, None),
            Err(error) => (None, Some(error)),
        };
        Ok(Self {
            config: config.to_owned(),
            client,
            jira,
            planner,
            prepared: None,
            restored,
            recovery_problem,
            fresh: true,
        })
    }
    pub fn snapshot(&self) -> &Arc<Snapshot> {
        self.planner.snapshot()
    }
    pub fn can_submit(&self) -> bool {
        self.fresh
            && self.prepared.is_none()
            && self.restored.is_none()
            && self.recovery_problem.is_none()
    }
    pub fn recovery_problem(&self) -> Option<ChangeError> {
        self.recovery_problem
    }
    pub fn pending_phase(&self) -> Option<WritePhase> {
        self.prepared
            .as_ref()
            .map(SavedChange::phase)
            .or_else(|| self.restored.as_ref().map(SavedRecovery::phase))
    }
    pub fn pending_draft(&self) -> Option<&Draft> {
        self.prepared
            .as_ref()
            .map(SavedChange::draft)
            .or_else(|| self.restored.as_ref().map(SavedRecovery::draft))
    }
    /// Only an authenticated restored record can authorize preserving these
    /// bytes during credential replacement. The vault must hold its own lease
    /// through the write and readback, using this exact expected value.
    pub fn credential_checkpoint(&self) -> Result<Option<&[u8]>, SessionError> {
        if let Some(error) = self.recovery_problem {
            return Err(error.into());
        }
        if self.prepared.is_some() {
            return Err(Error::WriteBlocked.into());
        }
        Ok(self
            .restored
            .as_ref()
            .and_then(SavedRecovery::stored_checkpoint))
    }
    pub async fn refresh(
        &mut self,
        range: DateRange,
        now: DateTime<Utc>,
    ) -> Result<(), SessionError> {
        self.fresh = false;
        self.planner.refresh(range, now).await?;
        self.fresh = true;
        Ok(())
    }
    pub async fn refresh_current_month_at(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<(), SessionError> {
        self.fresh = false;
        self.planner.refresh_current_month_at(now).await?;
        self.fresh = true;
        Ok(())
    }
    /// Persist and verify the journal before the original operation may send.
    /// A capture failure keeps the caller's editor and requires a fresh read.
    pub async fn submit(&mut self, draft: Draft) -> Result<DispatchReport, SessionError> {
        if !self.can_submit() {
            return Err(Error::WriteBlocked.into());
        }
        if draft.author() != &self.snapshot().account {
            return Err(Error::WrongConnection.into());
        }
        self.fresh = false;
        let verified = self.jira.myself().await.map_err(PlannerError::from)?;
        if verified.account != self.snapshot().account
            || verified.time_zone != self.snapshot().time_zone
        {
            return Err(PlannerError::ChangedIdentity.into());
        }
        self.prepared = Some(
            SavedChange::capture(self.client.clone(), self.jira.clone(), &self.config, draft)
                .await?,
        );
        self.dispatch_prepared().await
    }
    /// Explicit retry is available only for this instance's original Ready
    /// operation. Unconfirmed, terminal and restored operations cannot send.
    pub async fn dispatch_prepared(&mut self) -> Result<DispatchReport, SessionError> {
        let prepared = self.prepared.as_mut().ok_or(Error::WriteBlocked)?;
        self.fresh = false;
        let report = prepared.dispatch().await?;
        if matches!(report.phase, WritePhase::Confirmed | WritePhase::Rejected)
            && report.acknowledgement_durable
            && report.cleanup_problem.is_none()
        {
            self.prepared = None;
        }
        Ok(report)
    }
    /// Local retirement only. It never contacts Tempo or Jira.
    pub fn cleanup(&mut self) -> Result<(), SessionError> {
        self.fresh = false;
        if let Some(value) = &mut self.prepared {
            value.retry_cleanup()?;
            self.prepared = None;
        } else if let Some(value) = &mut self.restored {
            value.cleanup()?;
            self.restored = None;
        } else {
            return Err(Error::WriteBlocked.into());
        }
        Ok(())
    }
    /// Caller requires confirmation that a remote write may already exist.
    /// A successful discard still requires a successful read before a write.
    pub fn discard(&mut self) -> Result<(), SessionError> {
        self.fresh = false;
        if let Some(value) = &mut self.prepared {
            value.discard()?;
            self.prepared = None;
        } else if let Some(value) = &mut self.restored {
            value.discard()?;
            self.restored = None;
        } else {
            return Err(Error::WriteBlocked.into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
