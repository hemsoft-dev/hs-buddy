//! Owns a loaded account, complete reads and one durable user change.
//! All async operations and filesystem operations belong on an I/O worker.
use std::{path::PathBuf, sync::Arc};

use chrono::{DateTime, Utc};

use super::{
    AccountCalendar, Client, CreateDraft, EditDraft, Error, Id, Project, ReadScope, Task,
    TransportError, WriteKind, WritePhase, WriteRequest,
    change::{ChangeError, DispatchReport, SavedChange},
};

/// Immutable completed data remains available if a later refresh fails.
/// No Debug: contains private task and project text.
pub struct Dataset {
    pub account: Id,
    pub calendar: Arc<AccountCalendar>,
    pub tasks: Vec<Task>,
    pub projects: Vec<Project>,
    pub read_at: DateTime<Utc>,
    pub coverage: Coverage,
    /// A supplied qualification clock stays fixed; normal views use UTC now
    /// interpreted by the account calendar, independently of the host zone.
    pub fixed_now: Option<DateTime<Utc>>,
}

impl Dataset {
    pub fn now(&self) -> DateTime<Utc> {
        self.fixed_now.unwrap_or_else(Utc::now)
    }

    pub fn needs_day_refresh(&self, now: DateTime<Utc>) -> Result<bool, Error> {
        Ok(self.calendar.today_at(now)? != self.calendar.today_at(self.read_at)?)
    }
}

/// A completed task read describes exactly the scope it covers.
#[derive(Clone, PartialEq, Eq)]
pub enum Coverage {
    All,
    Calendar,
    Project(Id),
}

/// A verified project read remains usable when the all-task read exceeds its
/// bound. It contains no partially loaded task rows.
pub struct ProjectCatalog {
    pub projects: Vec<Project>,
}

pub enum TaskChange {
    Create(CreateDraft),
    Edit(Id, EditDraft),
    Complete(Id),
    Reopen(Id),
    Delete(Id),
}

/// Moving this owner into a worker prevents simultaneous refreshes and writes.
/// Reconnecting never silently replaces the identity of a pending command.
pub struct Planner {
    client: Client,
    account: Id,
    calendar: Arc<AccountCalendar>,
    config: PathBuf,
    fixed_now: Option<DateTime<Utc>>,
    data: Option<Arc<Dataset>>,
    catalog: Option<Arc<ProjectCatalog>>,
    coverage: Coverage,
    pending: Option<SavedChange>,
    recovery_blocked: bool,
    write_fresh: bool,
}

impl Planner {
    /// Verify identity and recovery without sending a task mutation.
    pub async fn connect(
        client: Client,
        config: PathBuf,
        fixed_now: Option<DateTime<Utc>>,
    ) -> Result<Self, ChangeError> {
        let profile = client.profile().await?;
        let calendar =
            Arc::new(AccountCalendar::from_profile(&profile).map_err(TransportError::from)?);
        let pending = SavedChange::recover_for_account(&client, &config, &profile.id).await?;
        Ok(Self {
            client,
            account: profile.id,
            calendar,
            config,
            fixed_now,
            data: None,
            catalog: None,
            coverage: Coverage::All,
            pending,
            recovery_blocked: false,
            write_fresh: false,
        })
    }

    pub fn account(&self) -> &Id {
        &self.account
    }

    pub fn calendar(&self) -> &AccountCalendar {
        &self.calendar
    }

    pub fn data(&self) -> Option<&Arc<Dataset>> {
        self.data.as_ref()
    }

    pub fn catalog(&self) -> Option<&Arc<ProjectCatalog>> {
        self.catalog.as_ref()
    }

    pub fn coverage(&self) -> &Coverage {
        &self.coverage
    }

    pub fn require_refresh(&mut self) {
        self.write_fresh = false;
    }

    pub fn can_create_in(&self, project: Option<&Id>) -> bool {
        self.can_submit() && project.is_none_or(|id| self.project_allows(id, WriteKind::Create))
    }

    pub fn can_change_task(&self, id: &Id, action: WriteKind) -> bool {
        self.can_submit()
            && self.data.as_ref().is_some_and(|data| {
                data.tasks.iter().any(|task| {
                    &task.id == id && !task.deleted && self.project_allows(&task.project_id, action)
                })
            })
    }

    fn project_allows(&self, id: &Id, action: WriteKind) -> bool {
        self.data.as_ref().is_some_and(|data| {
            data.projects
                .iter()
                .any(|project| &project.id == id && project.allows(action))
        })
    }

    pub fn pending(&self) -> Option<&SavedChange> {
        self.pending.as_ref()
    }

    pub fn recovery_blocked(&self) -> bool {
        self.recovery_blocked
    }

    /// A successful refresh is required after a known write or discarded
    /// uncertainty, so stale rows cannot become a second completion command.
    pub fn can_submit(&self) -> bool {
        self.data.is_some() && self.write_fresh && self.pending.is_none() && !self.recovery_blocked
    }

    /// Publish only a complete read belonging to the same account and calendar.
    /// This method never sends or retries a pending mutation.
    pub async fn refresh(&mut self) -> Result<Arc<Dataset>, TransportError> {
        self.refresh_scope(self.coverage.clone()).await
    }

    pub async fn refresh_scope(
        &mut self,
        coverage: Coverage,
    ) -> Result<Arc<Dataset>, TransportError> {
        self.refresh_scope_at(coverage, self.fixed_now.unwrap_or_else(Utc::now))
            .await
    }

    /// A replacement token need not load a project that no longer exists.
    /// Fall back for an unavailable project or an oversized broad scope, never
    /// for authentication or an incomplete read. Each read rechecks identity.
    pub async fn refresh_connection_scope(
        &mut self,
        preferred: Coverage,
    ) -> Result<Arc<Dataset>, TransportError> {
        let calendar = preferred == Coverage::Calendar;
        let broad = matches!(preferred, Coverage::All | Coverage::Calendar);
        let result = self.refresh_scope(preferred).await;
        let can_fall_back = matches!(
            &result,
            Err(TransportError::Domain(Error::ScopeUnavailable))
        ) || broad
            && matches!(&result, Err(TransportError::Domain(Error::ReadLimit)));
        if !can_fall_back {
            return result;
        }
        if !calendar {
            match self.refresh_scope(Coverage::Calendar).await {
                Err(TransportError::Domain(Error::ReadLimit)) => {}
                result => return result,
            }
        }
        let project = self
            .catalog
            .as_ref()
            .and_then(|catalog| {
                catalog
                    .projects
                    .iter()
                    .find(|project| !project.archived && !project.deleted)
            })
            .map(|project| project.id.clone());
        match project {
            Some(project) => self.refresh_scope(Coverage::Project(project)).await,
            None => Err(Error::ReadLimit.into()),
        }
    }

    /// The actual protected-token save shares recovery publication's lock and
    /// requires exactly the recovery state authenticated by this candidate.
    pub fn save_token(&self, token: &str) -> Result<(), super::credentials::CredentialError> {
        super::credentials::save_preserving_recovery_at(
            &self.config,
            token,
            self.pending.as_ref().and_then(SavedChange::recovery_bytes),
        )
    }

    /// A captured clock makes day-boundary reads testable. This only refreshes
    /// the existing scope; it never sends or retries a service mutation.
    pub async fn refresh_day_at(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<Option<Arc<Dataset>>, TransportError> {
        let Some(data) = &self.data else {
            return Ok(None);
        };
        if !data.needs_day_refresh(now)? {
            return Ok(None);
        }
        self.refresh_scope_at(self.coverage.clone(), now)
            .await
            .map(Some)
    }

    async fn refresh_scope_at(
        &mut self,
        coverage: Coverage,
        now: DateTime<Utc>,
    ) -> Result<Arc<Dataset>, TransportError> {
        self.write_fresh = false;
        let profile = self.client.profile().await?;
        if profile.id != self.account {
            return Err(Error::WrongConnection.into());
        }
        let calendar = Arc::new(AccountCalendar::from_profile(&profile)?);
        let projects = self.client.projects().await?;
        let scope = match &coverage {
            Coverage::All => ReadScope::Active,
            Coverage::Calendar => ReadScope::Filter("overdue | 7 days".into()),
            Coverage::Project(id) => {
                if !projects
                    .iter()
                    .any(|project| &project.id == id && !project.archived && !project.deleted)
                {
                    return Err(Error::ScopeUnavailable.into());
                }
                ReadScope::Project(id.clone())
            }
        };
        let tasks = self.client.tasks(&scope).await;
        if let Err(error) = &tasks
            && *error != TransportError::Domain(Error::ReadLimit)
        {
            return Err(*error);
        }
        let final_profile = self.client.profile().await?;
        if final_profile.id != self.account || final_profile.timezone != profile.timezone {
            return Err(Error::WrongConnection.into());
        }
        // Even an oversized task read leaves a complete, identity-checked
        // project catalog available for choosing a bounded project read.
        if matches!(
            &tasks,
            Ok(_) | Err(TransportError::Domain(Error::ReadLimit))
        ) {
            self.catalog = Some(Arc::new(ProjectCatalog {
                projects: projects.clone(),
            }));
        }
        let tasks = tasks?;
        let data = Arc::new(Dataset {
            account: self.account.clone(),
            calendar: calendar.clone(),
            projects,
            tasks,
            read_at: now,
            coverage: coverage.clone(),
            fixed_now: self.fixed_now.map(|_| now),
        });
        self.calendar = calendar;
        self.data = Some(data.clone());
        self.coverage = coverage;
        self.fixed_now = self.fixed_now.map(|_| now);
        self.write_fresh = true;
        Ok(data)
    }

    /// Validate a fresh draft against the loaded account, persist it, then send
    /// once. A failed capture blocks replacement until recovery is checked.
    pub async fn submit(&mut self, change: TaskChange) -> Result<DispatchReport, ChangeError> {
        if !self.can_submit() {
            return Err(TransportError::Domain(Error::WriteBlocked).into());
        }
        let request = self.request(change).map_err(TransportError::from)?;
        match SavedChange::capture_for_account(&self.client, &self.config, request, &self.account)
            .await
        {
            Ok(change) => self.pending = Some(change),
            Err(error) => {
                self.recovery_blocked = true;
                return Err(error);
            }
        }
        self.retry().await
    }

    fn request(&self, change: TaskChange) -> Result<WriteRequest, Error> {
        let data = self.data.as_ref().ok_or(Error::WriteBlocked)?;
        let scope = self.client.write_scope();
        if let TaskChange::Create(draft) = change {
            if draft.project.as_ref().is_some_and(|id| {
                !data
                    .projects
                    .iter()
                    .any(|project| &project.id == id && !project.archived && !project.deleted)
            }) {
                return Err(Error::InvalidDraft);
            }
            if !self.can_create_in(draft.project.as_ref()) {
                return Err(Error::WritePermission);
            }
            return WriteRequest::create(scope, &draft);
        }
        let target = match &change {
            TaskChange::Edit(id, _)
            | TaskChange::Complete(id)
            | TaskChange::Reopen(id)
            | TaskChange::Delete(id) => id,
            TaskChange::Create(_) => unreachable!(),
        };
        if !data
            .tasks
            .iter()
            .any(|task| &task.id == target && !task.deleted)
        {
            return Err(Error::InvalidDraft);
        }
        let action = match &change {
            TaskChange::Edit(..) => WriteKind::Edit,
            TaskChange::Complete(_) => WriteKind::Complete,
            TaskChange::Reopen(_) => WriteKind::Reopen,
            TaskChange::Delete(_) => WriteKind::Delete,
            TaskChange::Create(_) => unreachable!(),
        };
        if !self.can_change_task(target, action) {
            return Err(Error::WritePermission);
        }
        match change {
            TaskChange::Edit(id, draft) => WriteRequest::edit(scope, &id, &draft),
            TaskChange::Complete(id) => WriteRequest::complete(scope, &id),
            TaskChange::Reopen(id) => WriteRequest::reopen(scope, &id),
            TaskChange::Delete(id) => WriteRequest::delete(scope, &id),
            TaskChange::Create(_) => unreachable!(),
        }
    }

    /// Only an explicit user action retries the captured UUID and payload.
    pub async fn retry(&mut self) -> Result<DispatchReport, ChangeError> {
        let pending = self
            .pending
            .as_mut()
            .ok_or(TransportError::Domain(Error::WriteBlocked))?;
        let report = pending.dispatch(&self.client).await?;
        if matches!(report.phase, WritePhase::Confirmed | WritePhase::Rejected) {
            self.write_fresh = false;
        }
        if matches!(report.phase, WritePhase::Confirmed | WritePhase::Rejected)
            && report.cleanup_problem.is_none()
        {
            self.pending = None;
        }
        Ok(report)
    }

    /// Acknowledged commands only need local cleanup, never another POST.
    pub fn retry_cleanup(&mut self) -> Result<(), ChangeError> {
        self.pending
            .as_mut()
            .ok_or(TransportError::Domain(Error::WriteBlocked))?
            .retry_cleanup()?;
        self.pending = None;
        self.write_fresh = false;
        Ok(())
    }

    /// The view must first explain that the remote change may already exist.
    pub fn discard_pending(&mut self) -> Result<(), ChangeError> {
        self.pending
            .as_mut()
            .ok_or(TransportError::Domain(Error::WriteBlocked))?
            .discard()?;
        self.pending = None;
        self.write_fresh = false;
        Ok(())
    }

    /// Check uncertain storage without changing an acknowledged in-memory state.
    pub async fn check_recovery(&mut self) -> Result<(), ChangeError> {
        if self.pending.is_some() {
            return Err(TransportError::Domain(Error::WriteBlocked).into());
        }
        self.recovery_blocked = true;
        self.pending =
            SavedChange::recover_for_account(&self.client, &self.config, &self.account).await?;
        self.recovery_blocked = false;
        self.write_fresh = false;
        Ok(())
    }

    /// A replacement token must authenticate the original account. Failures
    /// leave this connection and its complete dataset untouched. Acknowledged
    /// cleanup must finish before reconnecting, preserving the known outcome.
    pub async fn reconnect(&mut self, client: Client) -> Result<(), ChangeError> {
        if self.pending.as_ref().is_some_and(|change| {
            matches!(change.phase(), WritePhase::Confirmed | WritePhase::Rejected)
        }) {
            return Err(TransportError::Domain(Error::WriteBlocked).into());
        }
        let profile = client.profile().await?;
        if profile.id != self.account {
            return Err(TransportError::Domain(Error::WrongConnection).into());
        }
        let calendar =
            Arc::new(AccountCalendar::from_profile(&profile).map_err(TransportError::from)?);
        let pending =
            SavedChange::recover_for_account(&client, &self.config, &self.account).await?;
        if self.pending.as_ref().is_some_and(|original| {
            pending.as_ref().is_none_or(|recovered| {
                recovered.request().command_id() != original.request().command_id()
                    || recovered.request().commands() != original.request().commands()
            })
        }) {
            return Err(super::storage::StorageError::Changed.into());
        }
        self.client = client;
        let _ = calendar;
        self.pending = pending;
        self.recovery_blocked = false;
        self.write_fresh = false;
        Ok(())
    }
}
