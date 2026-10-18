//! Read-only worker ownership. Publish a range only after all reads succeed.
//! Calendar dates come from the caller and service, never the host timezone.
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use super::{
    AccountId, Client, DateRange, Error, JiraClient, JiraError, TimesheetSummary, TransportError,
    summarize,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlannerError {
    InvalidProfile,
    ChangedIdentity,
    Jira(JiraError),
    Tempo(TransportError),
    Domain(Error),
}
impl std::fmt::Display for PlannerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidProfile => f.write_str("The Tempo configuration profile is unavailable. The existing timesheet is retained."),
            Self::ChangedIdentity => f.write_str("The Jira account or timezone changed. Reconnect before loading another range. The existing timesheet is retained."),
            Self::Jira(error) => error.fmt(f),
            Self::Tempo(error) => error.fmt(f),
            Self::Domain(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for PlannerError {}
impl From<JiraError> for PlannerError {
    fn from(value: JiraError) -> Self {
        Self::Jira(value)
    }
}
impl From<TransportError> for PlannerError {
    fn from(value: TransportError) -> Self {
        Self::Tempo(value)
    }
}
impl From<Error> for PlannerError {
    fn from(value: Error) -> Self {
        Self::Domain(value)
    }
}

/// No Debug or mutable access. Missing timezone metadata stays missing; callers
/// must resolve service calendar rules before deriving clock-based navigation.
pub struct Snapshot {
    pub account: AccountId,
    pub time_zone: Option<String>,
    pub range: DateRange,
    pub summary: TimesheetSummary,
    pub read_at: DateTime<Utc>,
    profile: String,
}
impl Snapshot {
    pub fn configuration_profile(&self) -> &str {
        &self.profile
    }
}

enum RangeSelection {
    Explicit(DateRange),
    CurrentMonth,
}

struct Connection {
    tempo: Arc<Client>,
    jira: Arc<JiraClient>,
    profile: String,
}
impl Connection {
    async fn read(
        &self,
        selection: RangeSelection,
        read_at: DateTime<Utc>,
        previous: Option<&Snapshot>,
    ) -> Result<Snapshot, PlannerError> {
        let selection = match selection {
            RangeSelection::Explicit(range) => {
                RangeSelection::Explicit(DateRange::new(range.start, range.end)?)
            }
            RangeSelection::CurrentMonth => RangeSelection::CurrentMonth,
        };
        tokio::time::timeout(Duration::from_secs(120), async {
            let before = self.jira.myself().await?;
            if previous.is_some_and(|old| {
                old.account != before.account
                    || old.time_zone != before.time_zone
                    || old.profile != self.profile
            }) {
                return Err(PlannerError::ChangedIdentity);
            }
            let range = match selection {
                RangeSelection::Explicit(range) => range,
                RangeSelection::CurrentMonth => {
                    super::AccountCalendar::from_zone(before.time_zone.as_deref())?
                        .current_month_at(read_at)?
                }
            };
            let rows = self.tempo.worklogs(before.account.clone(), range).await?;
            let schedules = self.tempo.schedule(before.account.clone(), range).await?;
            let after = self.jira.myself().await?;
            if before.account != after.account || before.time_zone != after.time_zone {
                return Err(PlannerError::ChangedIdentity);
            }
            let summary = summarize(&rows, &before.account, range, &schedules)?;
            Ok(Snapshot {
                account: before.account,
                time_zone: before.time_zone,
                range,
                summary,
                read_at,
                profile: self.profile.clone(),
            })
        })
        .await
        .map_err(|_| PlannerError::Domain(Error::IncompleteRead))?
    }
}

/// Move this controller into one I/O worker and return it with the outcome.
/// A candidate connection never mutates an existing controller or credential.
/// This controller exposes no mutation, automatic refresh, or recovery resend.
pub struct Planner {
    connection: Connection,
    snapshot: Arc<Snapshot>,
}
impl Planner {
    pub async fn connect(
        config: &Path,
        tempo: Arc<Client>,
        jira: Arc<JiraClient>,
        range: DateRange,
        read_at: DateTime<Utc>,
    ) -> Result<Self, PlannerError> {
        Self::connect_selected(
            config,
            tempo,
            jira,
            RangeSelection::Explicit(range),
            read_at,
        )
        .await
    }
    pub async fn connect_current_month_at(
        config: &Path,
        tempo: Arc<Client>,
        jira: Arc<JiraClient>,
        read_at: DateTime<Utc>,
    ) -> Result<Self, PlannerError> {
        Self::connect_selected(config, tempo, jira, RangeSelection::CurrentMonth, read_at).await
    }
    async fn connect_selected(
        config: &Path,
        tempo: Arc<Client>,
        jira: Arc<JiraClient>,
        range: RangeSelection,
        read_at: DateTime<Utc>,
    ) -> Result<Self, PlannerError> {
        if !config.is_absolute() || config.file_name().is_none() {
            return Err(PlannerError::InvalidProfile);
        }
        let profile = crate::pollen_credentials::account_for_path(config)
            .map_err(|_| PlannerError::InvalidProfile)?;
        let connection = Connection {
            tempo,
            jira,
            profile,
        };
        let snapshot = Arc::new(connection.read(range, read_at, None).await?);
        Ok(Self {
            connection,
            snapshot,
        })
    }
    pub fn snapshot(&self) -> &Arc<Snapshot> {
        &self.snapshot
    }
    pub async fn refresh(
        &mut self,
        range: DateRange,
        read_at: DateTime<Utc>,
    ) -> Result<(), PlannerError> {
        self.refresh_selected(RangeSelection::Explicit(range), read_at)
            .await
    }
    pub async fn refresh_current_month_at(
        &mut self,
        read_at: DateTime<Utc>,
    ) -> Result<(), PlannerError> {
        self.refresh_selected(RangeSelection::CurrentMonth, read_at)
            .await
    }
    async fn refresh_selected(
        &mut self,
        range: RangeSelection,
        read_at: DateTime<Utc>,
    ) -> Result<(), PlannerError> {
        let loaded = self
            .connection
            .read(range, read_at, Some(&self.snapshot))
            .await?;
        self.snapshot = Arc::new(loaded);
        Ok(())
    }
}

#[cfg(test)]
#[path = "planner_tests.rs"]
mod tests;
