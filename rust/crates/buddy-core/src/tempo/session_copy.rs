//! A copy plan is read-only, connection-bound and consumed once on confirmation.
use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};

use super::{Draft, Error, Planner, PlannerError, Session, SessionError, WritePhase};
use crate::tempo::{
    AccountCalendar, AccountId, StorageError, TransportError, Worklog, WorklogId, WriteScope,
};

const MAX_COPY_BYTES: usize = 16 * 1024 * 1024;

/// No Clone, Debug or Serialize. Confirmation consumes this exact read model.
pub struct CopyPlan {
    scope: WriteScope,
    profile: String,
    account: AccountId,
    origin: String,
    zone: Option<String>,
    date: NaiveDate,
    sources: Vec<Worklog>,
    seconds: u64,
}
impl CopyPlan {
    pub fn date(&self) -> NaiveDate {
        self.date
    }
    pub fn sources(&self) -> &[Worklog] {
        &self.sources
    }
    pub fn seconds(&self) -> u64 {
        self.seconds
    }
}

/// Confirmed entries are source/new-ID pairs. An uncertain current entry is
/// deliberately absent from unattempted; it must never become a retry list.
pub struct CopyReport {
    pub confirmed: Vec<(WorklogId, WorklogId)>,
    pub rejected: Option<WorklogId>,
    pub unconfirmed: Option<WorklogId>,
    pub unattempted: Vec<WorklogId>,
    pub problem: Option<SessionError>,
    pub transport_problem: Option<TransportError>,
    pub cleanup_problem: Option<StorageError>,
    pub refresh_problem: Option<SessionError>,
}
impl CopyReport {
    fn new(plan: &CopyPlan) -> Self {
        Self {
            confirmed: Vec::new(),
            rejected: None,
            unconfirmed: None,
            unattempted: plan.sources.iter().map(|row| row.id).collect(),
            problem: None,
            transport_problem: None,
            cleanup_problem: None,
            refresh_problem: None,
        }
    }
    pub fn complete(&self) -> bool {
        !self.confirmed.is_empty()
            && self.unattempted.is_empty()
            && self.rejected.is_none()
            && self.unconfirmed.is_none()
            && self.problem.is_none()
            && self.transport_problem.is_none()
            && self.cleanup_problem.is_none()
            && self.refresh_problem.is_none()
    }
}

/// Match the incumbent's 08:00 plus exact logged seconds, wrapping at midnight.
pub fn copy_start_time(seconds: u64) -> NaiveTime {
    let value = (8 * 3600 + seconds % 86400) % 86400;
    NaiveTime::from_num_seconds_from_midnight_opt(value as u32, 0).expect("bounded day seconds")
}

pub(super) fn displayed_baseline(displayed: &Worklog, expanded: &Worklog) -> Result<(), Error> {
    let mut known = expanded.clone();
    if displayed.attributes.is_none() {
        // Unknown attributes may be expanded; every displayed field remains a
        // full equality check, even if the service reuses its updated timestamp.
        known.attributes = None;
    }
    Draft::delete(displayed)?.check_update_baseline(&known)
}

impl Session {
    pub async fn prepare_copy(
        &mut self,
        ids: Vec<WorklogId>,
        now: DateTime<Utc>,
    ) -> Result<CopyPlan, SessionError> {
        if !self.can_submit() {
            return Err(Error::WriteBlocked.into());
        }
        if ids.is_empty() || ids.len() > crate::tempo::MAX_PAGE_ITEMS {
            return Err(Error::CopyLimit.into());
        }
        let mut unique = HashSet::new();
        let mut displayed = Vec::with_capacity(ids.len());
        for id in ids {
            if !unique.insert(id.get()) {
                return Err(Error::InvalidDraft.into());
            }
            let row = self
                .snapshot()
                .summary
                .days
                .iter()
                .flat_map(|day| &day.logged.worklogs)
                .find(|row| row.id == id)
                .ok_or(Error::WrongScope)?;
            displayed.push(row.clone());
        }
        self.fresh = false;
        // Each bounded source expansion has its own paced read budget. The
        // aggregate deadline must also allow the initial range read to finish.
        let count = u32::try_from(displayed.len()).map_err(|_| Error::CopyLimit)?;
        let deadline = crate::tempo::planner::RANGE_READ_TIMEOUT
            .saturating_add(crate::tempo::transport::READ_TIMEOUT.saturating_mul(count))
            .saturating_add(Duration::from_secs(5));
        let loaded = tokio::time::timeout(deadline, async {
            let target = Planner::connect_current_month_at(
                &self.config,
                self.client.clone(),
                self.jira.clone(),
                now,
            )
            .await?;
            let current = self.snapshot();
            let data = target.snapshot();
            if data.account != current.account || data.time_zone != current.time_zone {
                return Err(PlannerError::ChangedIdentity.into());
            }
            let date = AccountCalendar::from_zone(data.time_zone.as_deref())?.today_at(now)?;
            let mut sources = Vec::with_capacity(displayed.len());
            let mut bytes = 0usize;
            let mut seconds = 0u64;
            for row in displayed {
                let expanded = self.worklog_for_edit(row.id).await?;
                displayed_baseline(&row, &expanded)?;
                let body = Draft::copy_to_date(&expanded, date, copy_start_time(0))?.encode()?;
                bytes = bytes.checked_add(body.len()).ok_or(Error::CopyLimit)?;
                if bytes > MAX_COPY_BYTES {
                    return Err(Error::CopyLimit.into());
                }
                seconds = seconds
                    .checked_add(expanded.seconds)
                    .ok_or(Error::TotalOverflow)?;
                sources.push(expanded);
            }
            Ok::<_, SessionError>(CopyPlan {
                scope: self.client.write_scope(),
                profile: current.configuration_profile().to_owned(),
                account: current.account.clone(),
                origin: self.jira.origin().to_owned(),
                zone: current.time_zone.clone(),
                date,
                sources,
                seconds,
            })
        })
        .await
        .map_err(|_| SessionError::Read(PlannerError::Domain(Error::IncompleteRead)))?;
        if loaded.is_ok() {
            self.fresh = true;
        }
        loaded
    }

    async fn verify_copy(
        &mut self,
        plan: &CopyPlan,
        now: DateTime<Utc>,
    ) -> Result<(), SessionError> {
        if !self.can_submit() {
            return Err(Error::WriteBlocked.into());
        }
        let current = self.snapshot();
        if plan.scope != self.client.write_scope()
            || plan.profile != current.configuration_profile()
            || plan.account != current.account
            || plan.origin != self.jira.origin()
            || plan.zone != current.time_zone
        {
            return Err(Error::WrongConnection.into());
        }
        if AccountCalendar::from_zone(plan.zone.as_deref())?.today_at(now)? != plan.date {
            return Err(Error::ChangedWhileLoading.into());
        }
        self.refresh_current_month_at(now).await?;
        // Verify every approved expanded source before the first write. Source
        // changes are observed conflicts, not a claim of a server-side lock.
        for source in &plan.sources {
            let latest = self.worklog_for_edit(source.id).await?;
            Draft::delete(source)?.check_update_baseline(&latest)?;
        }
        Ok(())
    }

    pub async fn execute_copy(&mut self, plan: CopyPlan, now: DateTime<Utc>) -> CopyReport {
        let mut result = CopyReport::new(&plan);
        if let Err(error) = self.verify_copy(&plan, now).await {
            self.fresh = false;
            result.problem = Some(error);
            return result;
        }
        for source in plan.sources {
            let seconds = self
                .snapshot()
                .summary
                .days
                .iter()
                .find(|day| day.logged.date == plan.date)
                .map(|day| day.logged.seconds);
            let Some(seconds) = seconds else {
                result.problem = Some(Error::InvalidRange.into());
                break;
            };
            let draft = match Draft::copy_to_date(&source, plan.date, copy_start_time(seconds)) {
                Ok(draft) => draft,
                Err(error) => {
                    result.problem = Some(error.into());
                    break;
                }
            };
            let report = match self.submit(draft).await {
                Ok(report) => report,
                Err(error) => {
                    result.problem = Some(error);
                    break;
                }
            };
            result.unattempted.remove(0);
            result.transport_problem = report.transport_problem;
            result.cleanup_problem = report.cleanup_problem;
            match (report.phase, report.worklog) {
                (WritePhase::Confirmed, Some(created)) => {
                    result.confirmed.push((source.id, created))
                }
                (WritePhase::Rejected, _) => {
                    result.rejected = Some(source.id);
                    break;
                }
                _ => {
                    result.unconfirmed = Some(source.id);
                    break;
                }
            }
            if !report.acknowledgement_durable || report.cleanup_problem.is_some() {
                if result.cleanup_problem.is_none() {
                    result.problem = Some(Error::WriteBlocked.into());
                }
                break;
            }
            if let Err(error) = self.refresh_current_month_at(now).await {
                result.refresh_problem = Some(error);
                break;
            }
            if !self.can_submit() {
                result.problem = Some(Error::WriteBlocked.into());
                break;
            }
        }
        result
    }
}
