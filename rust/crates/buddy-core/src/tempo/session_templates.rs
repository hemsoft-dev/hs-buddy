//! Read-only templates retain their source connection and never replace the view.
use std::collections::HashMap;

use chrono::{DateTime, Utc};

use super::{DateRange, Error, Planner, PlannerError, Session, SessionError};
use crate::tempo::{AccountId, IssueId, Worklog, WorklogId, WriteScope};

pub struct Templates {
    scope: WriteScope,
    profile: String,
    account: AccountId,
    origin: String,
    zone: Option<String>,
    range: DateRange,
    rows: Vec<Worklog>,
}
impl Templates {
    pub fn range(&self) -> DateRange {
        self.range
    }
    pub fn rows(&self) -> &[Worklog] {
        &self.rows
    }
}

impl Session {
    pub async fn previous_month_templates(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Templates, SessionError> {
        let current = self.snapshot();
        let range = DateRange::month_containing(current.range.start)?.shift_month(-1)?;
        let loaded = Planner::connect(
            &self.config,
            self.client.clone(),
            self.jira.clone(),
            range,
            now,
        )
        .await?;
        let data = loaded.snapshot();
        if data.account != current.account || data.time_zone != current.time_zone {
            return Err(PlannerError::ChangedIdentity.into());
        }
        let mut latest = HashMap::<IssueId, Worklog>::new();
        for row in data
            .summary
            .days
            .iter()
            .flat_map(|day| &day.logged.worklogs)
        {
            let Some(issue) = row.issue else { continue };
            if latest.get(&issue).is_none_or(|old| {
                (row.date, row.start_time, row.id.get()) > (old.date, old.start_time, old.id.get())
            }) {
                latest.insert(issue, row.clone());
            }
        }
        let mut rows: Vec<_> = latest.into_values().collect();
        rows.sort_by_key(|row| row.issue.map(|id| id.get()));
        Ok(Templates {
            scope: self.client.write_scope(),
            profile: current.configuration_profile().to_owned(),
            account: current.account.clone(),
            origin: self.jira.origin().to_owned(),
            zone: current.time_zone.clone(),
            range,
            rows,
        })
    }

    pub async fn expand_template(
        &self,
        templates: &Templates,
        id: WorklogId,
    ) -> Result<Worklog, SessionError> {
        let current = self.snapshot();
        if templates.scope != self.client.write_scope()
            || templates.profile != current.configuration_profile()
            || templates.account != current.account
            || templates.origin != self.jira.origin()
            || templates.zone != current.time_zone
        {
            return Err(Error::WrongConnection.into());
        }
        let displayed = templates
            .rows
            .iter()
            .find(|row| row.id == id)
            .ok_or(Error::WrongScope)?;
        let before = self.jira.myself().await.map_err(PlannerError::from)?;
        if before.account != templates.account || before.time_zone != templates.zone {
            return Err(PlannerError::ChangedIdentity.into());
        }
        let expanded = self.worklog_for_edit(id).await?;
        super::copy::displayed_baseline(displayed, &expanded)?;
        let after = self.jira.myself().await.map_err(PlannerError::from)?;
        if before.account != after.account || before.time_zone != after.time_zone {
            return Err(PlannerError::ChangedIdentity.into());
        }
        Ok(expanded)
    }
}
